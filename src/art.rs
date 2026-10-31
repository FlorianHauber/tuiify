//! Album art. Two renderers:
//!  * half-block (`▀` with fg/bg colors): works in any truecolor terminal
//!  * kitty graphics protocol: real pixels in kitty / ghostty / WezTerm
//!
//! (Sixel is not implemented yet.)

use anyhow::Result;
use base64::{engine::general_purpose::STANDARD, Engine};
use ratatui::crossterm::cursor::MoveTo;
use ratatui::crossterm::queue;
use ratatui::style::Color;
use std::io::{self, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

static NEXT_ID: AtomicU32 = AtomicU32::new(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArtMode {
    Off,
    HalfBlock,
    Kitty,
}

pub fn detect(mode: &str) -> ArtMode {
    match mode.trim().to_lowercase().as_str() {
        "off" | "none" | "false" => ArtMode::Off,
        "halfblock" | "half-block" | "half" | "blocks" => ArtMode::HalfBlock,
        "kitty" => ArtMode::Kitty,
        _ => {
            let env = |k: &str| std::env::var(k).unwrap_or_default();
            let in_tmux = !env("TMUX").is_empty();
            let kitty_like = !env("KITTY_WINDOW_ID").is_empty()
                || env("TERM") == "xterm-kitty"
                || env("TERM_PROGRAM") == "ghostty"
                || env("TERM_PROGRAM") == "WezTerm"
                || !env("GHOSTTY_RESOURCES_DIR").is_empty();
            if kitty_like && !in_tmux {
                ArtMode::Kitty
            } else {
                ArtMode::HalfBlock
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct Art {
    pub id: u32,
    pub cols: u16,
    pub rows: u16,
    /// cols*rows cells of (top pixel color, bottom pixel color)
    pub blocks: Vec<(Color, Color)>,
    /// base64 PNG, only when the kitty protocol is in use
    pub png_b64: Option<String>,
}

/// Downloads the cover once; it can then be rendered at any size without another request.
pub async fn download(http: &reqwest::Client, url: &str) -> Result<Arc<Vec<u8>>> {
    let bytes = http.get(url).send().await?.error_for_status()?.bytes().await?;
    Ok(Arc::new(bytes.to_vec()))
}

/// Renders downloaded image bytes into a `cols` x `rows` cell grid (half-blocks), and when
/// `png_px` is given also into a PNG of that size for the kitty graphics protocol.
pub async fn render(
    bytes: Arc<Vec<u8>>,
    cols: u16,
    rows: u16,
    png_px: Option<u32>,
) -> Result<Art> {
    let art = tokio::task::spawn_blocking(move || -> Result<Art> {
        let img = image::load_from_memory(&bytes)?;
        let rgb = img.to_rgb8();
        let small = image::imageops::resize(
            &rgb,
            cols as u32,
            rows as u32 * 2,
            image::imageops::FilterType::Triangle,
        );
        let mut blocks = Vec::with_capacity(cols as usize * rows as usize);
        for y in 0..rows as u32 {
            for x in 0..cols as u32 {
                let top = small.get_pixel(x, y * 2);
                let bot = small.get_pixel(x, y * 2 + 1);
                blocks.push((
                    Color::Rgb(top.0[0], top.0[1], top.0[2]),
                    Color::Rgb(bot.0[0], bot.0[1], bot.0[2]),
                ));
            }
        }
        let png_b64 = match png_px {
            Some(px) => {
                let px = px.clamp(64, 1024);
                let sq = image::imageops::resize(&rgb, px, px, image::imageops::FilterType::Triangle);
                let mut buf: Vec<u8> = Vec::new();
                image::DynamicImage::ImageRgb8(sq)
                    .write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png)?;
                Some(STANDARD.encode(&buf))
            }
            None => None,
        };
        Ok(Art {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            cols,
            rows,
            blocks,
            png_b64,
        })
    })
    .await??;
    Ok(art)
}

/// Draws the image with the kitty graphics protocol at cell (x, y).
pub fn kitty_place<W: Write>(out: &mut W, art: &Art, x: u16, y: u16) -> io::Result<()> {
    let b64 = match &art.png_b64 {
        Some(b) => b,
        None => return Ok(()),
    };
    queue!(out, MoveTo(x, y))?;
    let chunks: Vec<&[u8]> = b64.as_bytes().chunks(4096).collect();
    for (i, ch) in chunks.iter().enumerate() {
        let more = if i + 1 < chunks.len() { 1 } else { 0 };
        if i == 0 {
            write!(
                out,
                "\x1b_Ga=T,f=100,t=d,i={},c={},r={},C=1,q=2,m={};",
                art.id, art.cols, art.rows, more
            )?;
        } else {
            write!(out, "\x1b_Gm={more};")?;
        }
        out.write_all(ch)?;
        out.write_all(b"\x1b\\")?;
    }
    out.flush()
}

/// Removes all kitty images.
pub fn kitty_clear<W: Write>(out: &mut W) -> io::Result<()> {
    out.write_all(b"\x1b_Ga=d,d=A,q=2\x1b\\")?;
    out.flush()
}
