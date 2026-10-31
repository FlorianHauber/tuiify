//! Rendering. Everything is drawn with ratatui; colors come from the theme in the config.

use crate::app::*;
use crate::art::{Art, ArtMode};
use crate::config::{lerp_color, Action, Theme, ACTIONS};
use crate::models::*;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Clear, HighlightSpacing, List, ListItem, Paragraph, Scrollbar,
    ScrollbarOrientation, ScrollbarState, Wrap,
};
use ratatui::Frame;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

// ───────────────────────────── helpers ─────────────────────────────

/// Truncates (with `…`) or pads `s` to exactly `w` terminal columns.
pub fn fit(s: &str, w: usize) -> String {
    if w == 0 {
        return String::new();
    }
    let total = UnicodeWidthStr::width(s);
    let mut out = String::new();
    if total <= w {
        out.push_str(s);
        out.push_str(&" ".repeat(w - total));
        return out;
    }
    let mut cur = 0usize;
    for ch in s.chars() {
        let cw = UnicodeWidthChar::width(ch).unwrap_or(0);
        if cur + cw > w - 1 {
            break;
        }
        out.push(ch);
        cur += cw;
    }
    out.push('…');
    cur += 1;
    if cur < w {
        out.push_str(&" ".repeat(w - cur));
    }
    out
}

pub fn fmt_time(ms: u64) -> String {
    let s = ms / 1000;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
    } else {
        format!("{:02}:{:02}", s / 60, s % 60)
    }
}

fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width);
    let h = h.min(area.height);
    Rect {
        x: area.x + (area.width - w) / 2,
        y: area.y + (area.height - h) / 2,
        width: w,
        height: h,
    }
}

fn centered_pct(area: Rect, px: u16, py: u16) -> Rect {
    let w = (area.width as u32 * px as u32 / 100) as u16;
    let h = (area.height as u32 * py as u32 / 100) as u16;
    centered(area, w, h)
}

fn block_line(app: &App, title: Line<'static>, focused: bool) -> Block<'static> {
    let th = &app.theme;
    Block::default()
        .borders(Borders::ALL)
        .border_type(app.border_type)
        .border_style(Style::default().fg(if focused { th.border_focus } else { th.border }))
        .title(title)
}

fn block(app: &App, title: String, focused: bool) -> Block<'static> {
    let style = Style::default().fg(app.theme.accent).add_modifier(Modifier::BOLD);
    block_line(app, Line::from(Span::styled(format!(" {title} "), style)), focused)
}

fn sel_style(th: &Theme) -> Style {
    Style::default()
        .bg(th.sel_bg)
        .fg(th.sel_fg)
        .add_modifier(Modifier::BOLD)
}

fn track_line(t: &Track, width: usize, th: &Theme, playing: Option<&str>) -> Line<'static> {
    let dim = Style::default().fg(th.dim);
    let is_playing = playing.map(|u| u == t.uri.as_str()).unwrap_or(false);
    let dur = fmt_time(t.duration_ms);
    let avail = width.saturating_sub(2 + 1 + dur.len());
    let (wt, wa, wl) = if avail >= 60 {
        let usable = avail - 4;
        let a = usable * 42 / 100;
        let b = usable * 30 / 100;
        (a, b, usable - a - b)
    } else if avail >= 12 {
        let usable = avail - 2;
        let a = usable * 58 / 100;
        (a, usable - a, 0)
    } else {
        (avail, 0, 0)
    };
    let title_style = if is_playing {
        Style::default().fg(th.playing).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(th.fg)
    };
    let mut spans = vec![
        Span::styled(if is_playing { "♪ " } else { "  " }, Style::default().fg(th.playing)),
        Span::styled(fit(&t.name, wt), title_style),
    ];
    if wa > 0 {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(fit(&t.artist_line(), wa), dim));
    }
    if wl > 0 {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(fit(&t.album, wl), dim));
    }
    spans.push(Span::raw(" "));
    spans.push(Span::styled(dur, dim));
    Line::from(spans)
}

fn row_line(item: &Item, width: usize, th: &Theme, playing: Option<&str>) -> Line<'static> {
    let dim = Style::default().fg(th.dim);
    let fg = Style::default().fg(th.fg);
    match item {
        Item::Track(t) => track_line(t, width, th, playing),
        Item::Album(a) => {
            let avail = width.saturating_sub(2 + 1 + 4);
            let (wn, wa) = if avail >= 30 {
                let usable = avail - 2;
                let n = usable * 60 / 100;
                (n, usable - n)
            } else {
                (avail, 0)
            };
            let mut spans = vec![Span::raw("  "), Span::styled(fit(&a.name, wn), fg)];
            if wa > 0 {
                spans.push(Span::raw("  "));
                spans.push(Span::styled(fit(&a.artist, wa), dim));
            }
            spans.push(Span::raw(" "));
            spans.push(Span::styled(fit(&a.year, 4), dim));
            Line::from(spans)
        }
        Item::Artist(a) => Line::from(vec![
            Span::raw("  "),
            Span::styled(fit(&a.name, width.saturating_sub(2)), fg),
        ]),
        Item::Playlist(p) => {
            let avail = width.saturating_sub(2 + 1 + 5);
            let (wn, wo) = if avail >= 30 {
                let usable = avail - 2;
                let n = usable * 65 / 100;
                (n, usable - n)
            } else {
                (avail, 0)
            };
            let mut spans = vec![Span::raw("  "), Span::styled(fit(&p.name, wn), fg)];
            if wo > 0 {
                spans.push(Span::raw("  "));
                spans.push(Span::styled(fit(&p.owner, wo), dim));
            }
            spans.push(Span::raw(" "));
            spans.push(Span::styled(format!("{:>5}", p.total), dim));
            Line::from(spans)
        }
    }
}

// ───────────────────────────── entry point ─────────────────────────────

pub fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();
    f.render_widget(
        Block::default().style(Style::default().bg(app.theme.bg).fg(app.theme.fg)),
        area,
    );
    app.rects = Rects::default();
    match app.screen {
        Screen::Login => draw_login(f, app, area),
        Screen::Main => {
            if app.fullscreen {
                draw_fullscreen(f, app, area);
            } else {
                draw_main(f, app, area);
            }
            match app.popup {
                Popup::None => {}
                Popup::Help => draw_help(f, app, area),
                Popup::Devices => draw_devices(f, app, area),
                Popup::Player => draw_player(f, app, area),
            }
        }
    }
}

// ───────────────────────────── login ─────────────────────────────

fn draw_login(f: &mut Frame, app: &mut App, area: Rect) {
    let th = app.theme;
    let acc = Style::default().fg(th.accent).add_modifier(Modifier::BOLD);
    let dim = Style::default().fg(th.dim);
    let err = Style::default().fg(th.error);
    let boxr = centered(area, 80, 24);
    let blk = block(app, "spotui — sign in".to_string(), true);
    let inner = blk.inner(boxr);
    f.render_widget(blk, boxr);

    let mut lines: Vec<Line> = Vec::new();
    match app.login.phase {
        LoginPhase::NeedClientId => {
            lines.push(Line::from(Span::styled("Welcome! spotui needs a (free) Spotify app to talk to your account.", acc)));
            lines.push(Line::from(""));
            lines.push(Line::from("1. Open https://developer.spotify.com/dashboard and create an app"));
            lines.push(Line::from(format!(
                "2. Add this Redirect URI:  http://127.0.0.1:{}/callback",
                app.cfg.general.redirect_port
            )));
            lines.push(Line::from("3. Tick \"Web API\", save, then copy the Client ID"));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Spotify rules (2026): the app owner needs Premium; Development Mode apps allow 5 users.",
                dim,
            )));
            lines.push(Line::from(""));
            lines.push(Line::from(vec![
                Span::styled("Client ID: ", acc),
                Span::raw(format!("{}▏", app.login.input)),
            ]));
            if let Some(e) = &app.login.error {
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(e.clone(), err)));
            }
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                format!("Enter: continue   Esc: quit   (saved to {})", app.cfg_path.display()),
                dim,
            )));
        }
        LoginPhase::Waiting | LoginPhase::Failed => {
            if app.login.phase == LoginPhase::Waiting {
                lines.push(Line::from(Span::styled("Waiting for you to approve spotui in your browser…", acc)));
            } else {
                lines.push(Line::from(Span::styled("Sign-in failed", err)));
            }
            lines.push(Line::from(""));
            lines.push(Line::from("If no browser opened, visit this link:"));
            lines.push(Line::from(Span::styled(app.login.url.clone(), Style::default().fg(th.accent))));
            if let Some(e) = &app.login.error {
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(e.clone(), err)));
            }
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "o open browser · y copy link · r restart · c change client id · q quit",
                dim,
            )));
        }
    }
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

// ───────────────────────────── shared widgets ─────────────────────────────

fn make_list<'a>(items: Vec<ListItem<'a>>, blk: Block<'a>, focused: bool, th: &Theme) -> List<'a> {
    let style = if focused { sel_style(th) } else { Style::default().bg(th.sel_bg) };
    List::new(items)
        .block(blk)
        .highlight_style(style)
        .highlight_symbol(if focused { "▌" } else { " " })
        .highlight_spacing(HighlightSpacing::Always)
}

/// Draws a thin scrollbar on the right border of a bordered list area.
fn draw_scrollbar(f: &mut Frame, list_area: Rect, total: usize, pos: usize, color: Color, enabled: bool) {
    if !enabled {
        return;
    }
    let h = list_area.height.saturating_sub(2) as usize;
    if h == 0 || total <= h {
        return;
    }
    let area = Rect {
        x: list_area.x,
        y: list_area.y + 1,
        width: list_area.width,
        height: h as u16,
    };
    let mut st = ScrollbarState::new(total).position(pos);
    let sb = Scrollbar::new(ScrollbarOrientation::VerticalRight)
        .begin_symbol(None)
        .end_symbol(None)
        .track_symbol(None)
        .style(Style::default().fg(color));
    f.render_stateful_widget(sb, area, &mut st);
}

/// Time + progress bar with a knob. Returns (line, label width incl. gap, bar width).
fn progress_line(progress: u64, duration: u64, w: usize, th: &Theme) -> (Line<'static>, u16, u16) {
    let dim = Style::default().fg(th.dim);
    let acc = Style::default().fg(th.accent);
    let l = fmt_time(progress);
    let r = fmt_time(duration);
    let bar_w = w.saturating_sub(l.len() + r.len() + 2).max(4);
    let dur = duration.max(1);
    let filled = (((bar_w as u64) * progress.min(dur)) / dur) as usize;
    let filled = filled.min(bar_w);
    let mut spans: Vec<Span<'static>> = vec![Span::styled(format!("{l} "), dim)];
    if filled >= bar_w {
        spans.push(Span::styled("━".repeat(bar_w), acc));
    } else {
        spans.push(Span::styled("━".repeat(filled), acc));
        spans.push(Span::styled("●", acc.add_modifier(Modifier::BOLD)));
        spans.push(Span::styled("─".repeat(bar_w - filled - 1), dim));
    }
    spans.push(Span::styled(format!(" {r}"), dim));
    (Line::from(spans), (l.len() + 1) as u16, bar_w as u16)
}

fn controls_line(app: &App, pb: &Option<Playback>, track: &Track, th: &Theme) -> Line<'static> {
    let dim = Style::default().fg(th.dim);
    let acc = Style::default().fg(th.accent);
    let shuffle = pb.as_ref().map(|p| p.shuffle).unwrap_or(false);
    let repeat = pb.as_ref().map(|p| p.repeat.clone()).unwrap_or_default();
    let dev = pb.as_ref().and_then(|p| p.device.as_ref());
    let vol = dev.and_then(|d| d.volume);
    let mut spans: Vec<Span<'static>> = Vec::new();
    spans.push(Span::styled("⇄ shuffle", if shuffle { acc } else { dim }));
    spans.push(Span::raw("   "));
    let (rtxt, ron) = match repeat.as_str() {
        "context" => ("↻ repeat all", true),
        "track" => ("↻ repeat one", true),
        _ => ("↻ repeat", false),
    };
    spans.push(Span::styled(rtxt, if ron { acc } else { dim }));
    spans.push(Span::raw("   "));
    match vol {
        Some(v) => {
            let filled = (((v as usize) * 8 + 50) / 100).min(8);
            spans.push(Span::styled("vol ", dim));
            spans.push(Span::styled("━".repeat(filled), acc));
            spans.push(Span::styled("─".repeat(8 - filled), dim));
            spans.push(Span::styled(format!(" {v}%"), dim));
        }
        None => spans.push(Span::styled("vol –", dim)),
    }
    if matches!(&app.liked, Some((id, true)) if *id == track.id) {
        spans.push(Span::raw("   "));
        spans.push(Span::styled("♥ liked", acc));
    }
    if let Some(d) = dev {
        if !d.name.is_empty() {
            spans.push(Span::raw("   "));
            spans.push(Span::styled(format!("· {}", d.name), dim));
        }
    }
    Line::from(spans)
}

fn lyrics_lines(app: &App, h: usize, th: &Theme) -> Vec<Line<'static>> {
    let dim = Style::default().fg(th.dim);
    let fg = Style::default().fg(th.fg);
    let acc = Style::default().fg(th.accent).add_modifier(Modifier::BOLD);
    let msg = |s: &str| vec![Line::from(Span::styled(s.to_string(), dim))];
    if !app.cfg.lyrics.enabled {
        return msg("Lyrics are disabled ([lyrics] enabled = false in the config)");
    }
    if app.current_track().is_none() {
        return msg("Nothing is playing");
    }
    match &app.lyrics.status {
        LyricsStatus::Idle | LyricsStatus::Loading => {
            msg(&format!("{} Fetching lyrics…", app.spinner()))
        }
        LyricsStatus::NotFound => msg("No lyrics found for this track"),
        LyricsStatus::Failed(e) => msg(&format!("Could not fetch lyrics: {e}")),
        LyricsStatus::Found => match &app.lyrics.lyrics {
            None => msg("No lyrics found for this track"),
            Some(ly) => {
                let (cur, start) = if ly.synced {
                    let t = (app.progress_ms() as i64 + app.cfg.lyrics.offset_ms).max(0) as u64;
                    let idx = ly.lines.partition_point(|(ts, _)| *ts <= t);
                    let cur = if idx == 0 { None } else { Some(idx - 1) };
                    (cur, cur.unwrap_or(0).saturating_sub(h / 2))
                } else {
                    (None, app.lyrics.scroll.min(ly.lines.len().saturating_sub(1)))
                };
                ly.lines
                    .iter()
                    .enumerate()
                    .skip(start)
                    .take(h)
                    .map(|(i, (_, text))| {
                        let style = if Some(i) == cur {
                            acc
                        } else if ly.synced && cur.map(|c| i < c).unwrap_or(false) {
                            dim
                        } else {
                            fg
                        };
                        Line::from(Span::styled(text.clone(), style))
                    })
                    .collect()
            }
        },
    }
}

fn bars_lines(app: &App, w: usize, h: usize, th: &Theme) -> Vec<Line<'static>> {
    if w == 0 || h == 0 || app.bars.is_empty() {
        return Vec::new();
    }
    let bw = app.cfg.visualizer.bar_width.max(1) as usize;
    let gap = app.cfg.visualizer.bar_gap as usize;
    let n = ((w + gap) / (bw + gap)).max(1);
    let total = app.bars.len();
    let vals: Vec<f32> = (0..n)
        .map(|i| {
            let from = i * total / n;
            let to = ((i + 1) * total / n).max(from + 1).min(total);
            app.bars[from..to].iter().cloned().fold(0.0f32, f32::max)
        })
        .collect();
    const BLOCKS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    let used = (n * (bw + gap)).saturating_sub(gap);
    let pad = w.saturating_sub(used) / 2;
    let mut lines: Vec<Line<'static>> = Vec::with_capacity(h);
    for row in 0..h {
        let rb = h - 1 - row;
        let color = lerp_color(th.viz_low, th.viz_high, rb as f32 / h as f32);
        let mut spans: Vec<Span<'static>> = vec![Span::raw(" ".repeat(pad))];
        for (i, v) in vals.iter().enumerate() {
            let eighths = (v.clamp(0.0, 1.0) * h as f32 * 8.0) as usize;
            let cell = eighths.saturating_sub(rb * 8).min(8);
            let ch = if cell == 0 { ' ' } else { BLOCKS[cell - 1] };
            spans.push(Span::styled(ch.to_string().repeat(bw), Style::default().fg(color)));
            if i + 1 < n && gap > 0 {
                spans.push(Span::raw(" ".repeat(gap)));
            }
        }
        lines.push(Line::from(spans));
    }
    lines
}

/// Paints a half-block cover into `area`.
fn draw_art_cells(f: &mut Frame, area: Rect, a: &Art) {
    let cols = (a.cols as usize).min(area.width as usize);
    let rows = (a.rows as usize).min(area.height as usize);
    let mut lines: Vec<Line> = Vec::with_capacity(rows);
    for y in 0..rows {
        let mut spans: Vec<Span> = Vec::with_capacity(cols);
        for x in 0..cols {
            let (top, bot) = a.blocks[y * a.cols as usize + x];
            spans.push(Span::styled("▀", Style::default().fg(top).bg(bot)));
        }
        lines.push(Line::from(spans));
    }
    f.render_widget(Paragraph::new(lines), area);
}

// ───────────────────────────── main layout ─────────────────────────────

fn draw_main(f: &mut Frame, app: &mut App, area: Rect) {
    let (_, art_rows) = app.art_dims();
    let art_on = app.art_mode != ArtMode::Off;
    let inner_h: u16 = if art_on { art_rows.max(5) } else { 5 };
    let bar_h = inner_h + 2;
    let header_h: u16 = if app.cfg.ui.show_header { 1 } else { 0 };
    let help_h: u16 = if app.cfg.general.show_help_bar { 1 } else { 0 };

    let rows = Layout::vertical([
        Constraint::Length(header_h),
        Constraint::Min(4),
        Constraint::Length(bar_h),
        Constraint::Length(help_h),
    ])
    .split(area);

    if header_h > 0 {
        draw_header(f, app, rows[0]);
    }
    let top = rows[1];
    let show_sidebar = app.cfg.general.show_sidebar && top.width >= 60;
    let main_area = if show_sidebar {
        let w = app.cfg.general.sidebar_width.clamp(12, top.width / 2);
        let cols = Layout::horizontal([Constraint::Length(w), Constraint::Min(10)]).split(top);
        draw_sidebar(f, app, cols[0]);
        cols[1]
    } else {
        top
    };

    match app.view {
        View::Browse | View::Search => draw_browser(f, app, main_area),
        View::Queue => draw_queue(f, app, main_area),
        View::Lyrics => draw_lyrics(f, app, main_area),
        View::Visualizer => draw_viz(f, app, main_area),
    }
    draw_playbar(f, app, rows[2]);
    if help_h > 0 {
        draw_helpbar(f, app, rows[3]);
    }
}

fn draw_header(f: &mut Frame, app: &App, area: Rect) {
    let th = app.theme;
    let dim = Style::default().fg(th.dim);
    let badge = Style::default()
        .bg(th.accent)
        .fg(Color::Black)
        .add_modifier(Modifier::BOLD);
    let mut spans: Vec<Span<'static>> = vec![Span::styled(" ♫ spotui ", badge), Span::raw("  ")];
    let tabs: [(&str, Option<View>, Action); 6] = [
        ("Library", Some(View::Browse), Action::ViewBrowse),
        ("Search", Some(View::Search), Action::Search),
        ("Queue", Some(View::Queue), Action::ViewQueue),
        ("Lyrics", Some(View::Lyrics), Action::ViewLyrics),
        ("Visualizer", Some(View::Visualizer), Action::ViewVisualizer),
        ("Fullscreen", None, Action::Fullscreen),
    ];
    for (label, view, action) in tabs {
        let active = view.map(|v| v == app.view).unwrap_or(false);
        let key = app.keymap.first(action);
        let (name_style, key_style) = if active {
            (
                Style::default().fg(th.accent).bg(th.sel_bg).add_modifier(Modifier::BOLD),
                Style::default().fg(th.dim).bg(th.sel_bg),
            )
        } else {
            (Style::default().fg(th.fg), dim)
        };
        spans.push(Span::styled(format!(" {label} "), name_style));
        spans.push(Span::styled(format!("{key} "), key_style));
    }

    let mut right: Vec<Span<'static>> = Vec::new();
    if app.cfg.player.enabled {
        let item = match &app.player_state {
            PlayerState::Off => None,
            PlayerState::Ready => Some((th.accent, "player ready".to_string())),
            PlayerState::Starting | PlayerState::Connecting => {
                Some((Color::Yellow, "player starting…".to_string()))
            }
            PlayerState::NeedsLogin => Some((Color::Yellow, "player sign-in: ctrl+p".to_string())),
            PlayerState::Failed(_) => Some((th.error, "player failed: ctrl+p".to_string())),
        };
        if let Some((color, text)) = item {
            right.push(Span::styled("● ", Style::default().fg(color)));
            right.push(Span::styled(format!("{text} "), dim));
        }
    }
    let lw: usize = spans.iter().map(|s| UnicodeWidthStr::width(&*s.content)).sum();
    let rw: usize = right.iter().map(|s| UnicodeWidthStr::width(&*s.content)).sum();
    let pad = (area.width as usize).saturating_sub(lw + rw);
    if pad >= 1 {
        spans.push(Span::raw(" ".repeat(pad)));
        spans.extend(right);
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn sidebar_icon(e: &SidebarEntry) -> (&'static str, bool) {
    match e {
        SidebarEntry::Liked => ("♥", true),
        SidebarEntry::Recent => ("↺", true),
        SidebarEntry::Top => ("★", true),
        SidebarEntry::Albums => ("◉", true),
        SidebarEntry::Artists => ("♪", true),
        SidebarEntry::Playlist(_) => ("≡", false),
    }
}

fn draw_sidebar(f: &mut Frame, app: &mut App, area: Rect) {
    let th = app.theme;
    let focused = app.focus == Focus::Sidebar;
    let blk = block(app, "Library".to_string(), focused);
    let inner = blk.inner(area);
    app.rects.sidebar = inner;
    let w = inner.width.saturating_sub(1) as usize;
    let icons = app.cfg.ui.icons;
    let sb_on = app.cfg.ui.scrollbar;
    let items: Vec<ListItem> = app
        .sidebar
        .iter()
        .map(|e| {
            let (icon, accent) = sidebar_icon(e);
            let icon_style = if accent {
                Style::default().fg(th.accent)
            } else {
                Style::default().fg(th.dim)
            };
            let name_w = w.saturating_sub(if icons { 3 } else { 1 });
            let mut spans: Vec<Span> = vec![Span::raw(" ")];
            if icons {
                spans.push(Span::styled(icon, icon_style));
                spans.push(Span::raw(" "));
            }
            spans.push(Span::styled(fit(&e.label(), name_w), Style::default().fg(th.fg)));
            ListItem::new(Line::from(spans))
        })
        .collect();
    let total = items.len();
    let list = make_list(items, blk, focused, &th);
    f.render_stateful_widget(list, area, &mut app.sidebar_state);
    let sel = app.sidebar_state.selected().unwrap_or(0);
    draw_scrollbar(f, area, total, sel, th.accent, sb_on);
}

fn draw_search_box(f: &mut Frame, app: &mut App, area: Rect) {
    let th = app.theme;
    let mut spans = vec![Span::styled(
        " Search ",
        Style::default().fg(th.accent).add_modifier(Modifier::BOLD),
    )];
    for k in SearchKind::ALL {
        let style = if k == app.search.kind {
            Style::default().fg(th.accent).bg(th.sel_bg).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(th.dim)
        };
        spans.push(Span::styled(format!(" {} ", k.label()), style));
    }
    let blk = block_line(app, Line::from(spans), app.search.typing);
    let inner = blk.inner(area);
    f.render_widget(blk, area);
    let line = if app.search.input.is_empty() && !app.search.typing {
        Line::from(Span::styled(
            " press / to type · Tab changes the type · Enter on a playlist plays it",
            Style::default().fg(th.dim),
        ))
    } else {
        let cursor = if app.search.typing { "▏" } else { "" };
        Line::from(vec![
            Span::styled(" ⌕ ", Style::default().fg(th.accent)),
            Span::raw(format!("{}{}", app.search.input, cursor)),
        ])
    };
    f.render_widget(Paragraph::new(line), inner);
}

fn draw_browser(f: &mut Frame, app: &mut App, area: Rect) {
    let th = app.theme;
    let dim = Style::default().fg(th.dim);
    let is_search = app.view == View::Search;
    let focused = app.focus == Focus::Main;
    let spin = app.spinner();
    let sb_on = app.cfg.ui.scrollbar;

    let list_area = if is_search {
        let parts = Layout::vertical([Constraint::Length(3), Constraint::Min(3)]).split(area);
        draw_search_box(f, app, parts[0]);
        parts[1]
    } else {
        area
    };

    let playing_uri = app.current_track().map(|t| t.uri.clone());
    let (title, loading) = {
        let stack = if is_search { &app.search_stack } else { &app.browse };
        let t = stack
            .iter()
            .map(|c| c.title.as_str())
            .collect::<Vec<_>>()
            .join(" › ");
        let t = if t.is_empty() {
            if is_search { "Results".to_string() } else { "Browse".to_string() }
        } else {
            t
        };
        (t, stack.last().map(|c| c.loading).unwrap_or(false))
    };
    let title = if loading { format!("{title} {spin}") } else { title };
    let blk = block(app, title, focused);
    let inner = blk.inner(list_area);
    app.rects.list = inner;
    let w = inner.width.saturating_sub(1) as usize;

    let stack = if is_search { &mut app.search_stack } else { &mut app.browse };
    match stack.last_mut() {
        None => {
            let msg = if is_search {
                "Press / and type to search Spotify".to_string()
            } else {
                format!("{spin} Loading…")
            };
            f.render_widget(Paragraph::new(msg).style(dim).block(blk), list_area);
        }
        Some(c) => {
            if let Some(e) = &c.error {
                f.render_widget(
                    Paragraph::new(e.clone())
                        .style(Style::default().fg(th.error))
                        .wrap(Wrap { trim: false })
                        .block(blk),
                    list_area,
                );
            } else if c.items.is_empty() {
                let msg = if c.loading {
                    format!("{spin} Loading…")
                } else {
                    "Nothing here".to_string()
                };
                f.render_widget(
                    Paragraph::new(msg).style(dim).wrap(Wrap { trim: false }).block(blk),
                    list_area,
                );
            } else {
                let items: Vec<ListItem> = c
                    .items
                    .iter()
                    .map(|it| ListItem::new(row_line(it, w, &th, playing_uri.as_deref())))
                    .collect();
                let list = make_list(items, blk, focused, &th);
                f.render_stateful_widget(list, list_area, &mut c.state);
                let total = c.items.len();
                let sel = c.state.selected().unwrap_or(0);
                draw_scrollbar(f, list_area, total, sel, th.accent, sb_on);
            }
        }
    }
}

fn draw_queue(f: &mut Frame, app: &mut App, area: Rect) {
    let th = app.theme;
    let dim = Style::default().fg(th.dim);
    let acc = Style::default().fg(th.accent).add_modifier(Modifier::BOLD);
    let focused = app.focus == Focus::Main;
    let spin = app.spinner();
    let sb_on = app.cfg.ui.scrollbar;
    let parts = Layout::vertical([Constraint::Length(4), Constraint::Min(3)]).split(area);

    // "now playing" card
    let card = block(app, "Now playing".to_string(), false);
    let card_inner = card.inner(parts[0]);
    f.render_widget(card, parts[0]);
    let cur = app.current_track().cloned().or_else(|| app.queue.current.clone());
    let card_lines: Vec<Line> = match cur {
        Some(t) => vec![
            Line::from(vec![
                Span::styled("▶ ", Style::default().fg(th.accent)),
                Span::styled(
                    fit(&t.name, (card_inner.width as usize).saturating_sub(2)).trim_end().to_string(),
                    acc,
                ),
            ]),
            Line::from(Span::styled(
                fit(
                    &format!("  {}  ·  {}", t.artist_line(), t.album),
                    card_inner.width as usize,
                )
                .trim_end()
                .to_string(),
                dim,
            )),
        ],
        None => vec![Line::from(Span::styled("Nothing is playing", dim))],
    };
    f.render_widget(Paragraph::new(card_lines), card_inner);

    // upcoming tracks
    let mut title = format!("Next up · {}", app.queue.items.len());
    if app.queue.loading {
        title = format!("{title} {spin}");
    }
    let blk = block(app, title, focused);
    let inner = blk.inner(parts[1]);
    app.rects.list = inner;
    let w = inner.width.saturating_sub(1) as usize;

    if let Some(e) = &app.queue.error {
        f.render_widget(
            Paragraph::new(e.clone()).style(Style::default().fg(th.error)).block(blk),
            parts[1],
        );
        return;
    }
    if app.queue.items.is_empty() {
        let msg = if app.queue.loading {
            format!("{spin} Loading…")
        } else {
            "The queue is empty. Press `a` on a track to add it.".to_string()
        };
        f.render_widget(Paragraph::new(msg).style(dim).block(blk), parts[1]);
        return;
    }
    let items: Vec<ListItem> = app
        .queue
        .items
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let mut spans = vec![Span::styled(format!("{:>3} ", i + 1), dim)];
            spans.extend(track_line(t, w.saturating_sub(4), &th, None).spans);
            ListItem::new(Line::from(spans))
        })
        .collect();
    let total = items.len();
    let list = make_list(items, blk, focused, &th);
    f.render_stateful_widget(list, parts[1], &mut app.queue.state);
    let sel = app.queue.state.selected().unwrap_or(0);
    draw_scrollbar(f, parts[1], total, sel, th.accent, sb_on);
}

fn draw_lyrics(f: &mut Frame, app: &mut App, area: Rect) {
    let th = app.theme;
    let title = match app.current_track() {
        Some(t) => format!("Lyrics — {} · {}", t.name, t.artist_line()),
        None => "Lyrics".to_string(),
    };
    let blk = block(app, title, true);
    let inner = blk.inner(area);
    f.render_widget(blk, area);
    let lines = lyrics_lines(app, inner.height as usize, &th);
    f.render_widget(Paragraph::new(lines).alignment(Alignment::Center), inner);
}

fn draw_viz(f: &mut Frame, app: &mut App, area: Rect) {
    let th = app.theme;
    let dim = Style::default().fg(th.dim);
    let blk = block(app, "Visualizer".to_string(), true);
    let inner = blk.inner(area);
    f.render_widget(blk, area);

    if !app.cfg.visualizer.enabled {
        f.render_widget(
            Paragraph::new("The visualizer is disabled ([visualizer] enabled = false)").style(dim),
            inner,
        );
        return;
    }
    if let Some(e) = &app.viz_error {
        f.render_widget(
            Paragraph::new(e.clone())
                .style(Style::default().fg(th.error))
                .wrap(Wrap { trim: false }),
            inner,
        );
        return;
    }
    if app.bars.is_empty() {
        f.render_widget(
            Paragraph::new(format!(
                "{} Waiting for audio… (only audio playing on this computer can be visualized)",
                app.spinner()
            ))
            .style(dim),
            inner,
        );
        return;
    }
    let lines = bars_lines(app, inner.width as usize, inner.height as usize, &th);
    f.render_widget(Paragraph::new(lines), inner);
}

// ───────────────────────────── playbar ─────────────────────────────

fn draw_playbar(f: &mut Frame, app: &mut App, area: Rect) {
    let th = app.theme;
    let dim = Style::default().fg(th.dim);
    let acc = Style::default().fg(th.accent);
    let badge = app.player_badge();
    let title = if badge.is_empty() {
        "Now Playing".to_string()
    } else {
        format!("Now Playing · {badge}")
    };
    let blk = block(app, title, false);
    let inner = blk.inner(area);
    f.render_widget(blk, area);
    if inner.width < 12 || inner.height < 1 {
        return;
    }

    let (acols, arows) = app.art_dims();
    let show_art = app.art_mode != ArtMode::Off && inner.width > acols + 24;
    let text_x = if show_art { inner.x + acols + 2 } else { inner.x + 1 };
    let text_area = Rect {
        x: text_x,
        y: inner.y,
        width: (inner.x + inner.width).saturating_sub(text_x),
        height: inner.height,
    };

    if show_art {
        let ar = Rect {
            x: inner.x + 1,
            y: inner.y,
            width: acols.min(inner.width),
            height: arows.min(inner.height),
        };
        match &app.art {
            Some(a) => {
                if app.art_mode == ArtMode::HalfBlock {
                    draw_art_cells(f, ar, a);
                } else if app.popup == Popup::None {
                    // kitty: the image itself is placed after the frame is drawn
                    app.rects.art = Some(Rect {
                        x: ar.x,
                        y: ar.y,
                        width: a.cols.min(ar.width),
                        height: a.rows.min(ar.height),
                    });
                }
            }
            None => {
                f.render_widget(Paragraph::new("♪").style(dim).alignment(Alignment::Center), ar);
            }
        }
    }

    let w = text_area.width as usize;
    let pad = (text_area.height.saturating_sub(5) / 2) as usize;
    let mut lines: Vec<Line> = Vec::new();
    for _ in 0..pad {
        lines.push(Line::from(""));
    }
    let pb = app.playback.clone();
    let progress = app.progress_ms();

    match pb.as_ref().and_then(|p| p.track.as_ref()) {
        Some(t) => {
            let playing = pb.as_ref().map(|p| p.is_playing).unwrap_or(false);
            let icon = if playing { "▶ " } else { "❚❚ " };
            lines.push(Line::from(vec![
                Span::styled(icon, acc),
                Span::styled(
                    fit(&t.name, w.saturating_sub(3)).trim_end().to_string(),
                    Style::default().fg(th.fg).add_modifier(Modifier::BOLD),
                ),
            ]));
            let mut sub = vec![Span::styled(
                fit(&t.artist_line(), w).trim_end().to_string(),
                Style::default().fg(th.fg),
            )];
            if !t.album.is_empty() {
                sub.push(Span::styled(format!("  ·  {}", t.album), dim));
            }
            lines.push(Line::from(sub));
            lines.push(Line::from(""));

            let (pl, lbl, bar_w) = progress_line(progress, t.duration_ms, w, &th);
            app.rects.progress = Rect {
                x: text_area.x + lbl,
                y: text_area.y + pad as u16 + 3,
                width: bar_w,
                height: 1,
            };
            lines.push(pl);
            lines.push(controls_line(app, &pb, t, &th));
        }
        None => {
            lines.push(Line::from(Span::styled(
                "Nothing playing",
                Style::default().fg(th.fg).add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(Span::styled(
                "Pick a track from the library, or press `d` to choose a Spotify device",
                dim,
            )));
        }
    }
    f.render_widget(Paragraph::new(lines), text_area);
}

// ───────────────────────────── fullscreen ─────────────────────────────

fn draw_fullscreen(f: &mut Frame, app: &mut App, area: Rect) {
    let th = app.theme;
    let dim = Style::default().fg(th.dim);
    let acc = Style::default().fg(th.accent);
    let help_h: u16 = if app.cfg.general.show_help_bar { 1 } else { 0 };
    let rows = Layout::vertical([Constraint::Min(6), Constraint::Length(help_h)]).split(area);
    let body = rows[0];

    let badge = app.player_badge();
    let mut title = vec![Span::styled(
        " ♫ Now Playing ",
        Style::default().fg(th.accent).add_modifier(Modifier::BOLD),
    )];
    if !badge.is_empty() {
        title.push(Span::styled(format!("· {badge} "), dim));
    }
    let leave = app.keymap.first(Action::Fullscreen);
    title.push(Span::styled(format!("· {leave} to leave "), dim));
    let blk = block_line(app, Line::from(title), true);
    let inner = blk.inner(body);
    f.render_widget(blk, body);
    if help_h > 0 {
        draw_helpbar(f, app, rows[1]);
    }
    app.full_want = None;

    if inner.width < 24 || inner.height < 8 {
        f.render_widget(Paragraph::new("Terminal too small for fullscreen").style(dim), inner);
        return;
    }
    let pb = app.playback.clone();
    let track = match pb.as_ref().and_then(|p| p.track.clone()) {
        Some(t) => t,
        None => {
            f.render_widget(
                Paragraph::new("Nothing is playing").style(dim).alignment(Alignment::Center),
                inner,
            );
            return;
        }
    };
    let progress = app.progress_ms();

    // optional visualizer strip at the bottom
    let viz_ok = app.cfg.visualizer.enabled
        && app.viz_error.is_none()
        && !app.bars.is_empty()
        && inner.height >= 22;
    let viz_h: u16 = if viz_ok { 5 } else { 0 };
    let main_h = inner.height.saturating_sub(viz_h);
    let main = Rect { x: inner.x, y: inner.y, width: inner.width, height: main_h };
    let viz_rect = Rect {
        x: inner.x + 1,
        y: inner.y + main_h,
        width: inner.width.saturating_sub(2),
        height: viz_h,
    };

    // cover size
    let max_cols = app.cfg.ui.fullscreen_art_max.clamp(12, 120);
    let by_h = main.height.saturating_sub(2) * 2;
    let by_w = main.width * 2 / 5;
    let mut cols = by_h.min(by_w).min(max_cols);
    cols -= cols % 2;
    let art_on = app.art_mode != ArtMode::Off && main.width >= 56 && cols >= 12;
    let rows_n = cols / 2;
    if art_on {
        app.full_want = Some((cols, rows_n));
    }
    let left_w = if art_on { (cols + 4).min(main.width) } else { 0 };
    let left = Rect { x: main.x, y: main.y, width: left_w, height: main.height };
    let right = Rect {
        x: main.x + left_w,
        y: main.y,
        width: main.width.saturating_sub(left_w),
        height: main.height,
    };

    if art_on {
        let ar = Rect {
            x: left.x + 2,
            y: left.y + 1,
            width: cols.min(left.width.saturating_sub(2)),
            height: rows_n.min(left.height.saturating_sub(1)),
        };
        match &app.art_full {
            Some(a) => {
                if app.art_mode == ArtMode::HalfBlock {
                    draw_art_cells(f, ar, a);
                } else if app.popup == Popup::None {
                    app.rects.art = Some(Rect {
                        x: ar.x,
                        y: ar.y,
                        width: a.cols.min(ar.width),
                        height: a.rows.min(ar.height),
                    });
                }
            }
            None => {
                f.render_widget(
                    Paragraph::new(app.spinner()).style(dim).alignment(Alignment::Center),
                    ar,
                );
            }
        }

        // "Up next" under the cover
        let y0 = left.y + 1 + rows_n + 1;
        let avail = (left.y + left.height).saturating_sub(y0);
        if avail >= 3 && !app.queue.items.is_empty() {
            let w = left.width.saturating_sub(3) as usize;
            let mut ls: Vec<Line> = vec![Line::from(Span::styled(
                "Up next",
                acc.add_modifier(Modifier::BOLD),
            ))];
            for t in app.queue.items.iter().take((avail - 1) as usize) {
                let text = format!("{} — {}", t.name, t.artist_line());
                ls.push(Line::from(Span::styled(
                    fit(&text, w).trim_end().to_string(),
                    dim,
                )));
            }
            f.render_widget(
                Paragraph::new(ls),
                Rect {
                    x: left.x + 2,
                    y: y0,
                    width: left.width.saturating_sub(2),
                    height: avail,
                },
            );
        }
    }

    // info + lyrics
    let r = Rect {
        x: right.x + 2,
        y: right.y + 1,
        width: right.width.saturating_sub(4),
        height: right.height.saturating_sub(1),
    };
    if r.width >= 10 && r.height >= 6 {
        let parts = Layout::vertical([
            Constraint::Length(7u16.min(r.height)),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .split(r);
        let w = r.width as usize;
        let playing = pb.as_ref().map(|p| p.is_playing).unwrap_or(false);
        let mut info: Vec<Line> = Vec::new();
        info.push(Line::from(vec![
            Span::styled(if playing { "▶ " } else { "❚❚ " }, acc),
            Span::styled(
                fit(&track.name, w.saturating_sub(3)).trim_end().to_string(),
                Style::default().fg(th.fg).add_modifier(Modifier::BOLD),
            ),
        ]));
        info.push(Line::from(Span::styled(
            fit(&track.artist_line(), w).trim_end().to_string(),
            acc,
        )));
        info.push(Line::from(Span::styled(
            fit(&track.album, w).trim_end().to_string(),
            dim,
        )));
        info.push(Line::from(""));
        let (pl, lbl, bar_w) = progress_line(progress, track.duration_ms, w, &th);
        app.rects.progress = Rect { x: r.x + lbl, y: parts[0].y + 4, width: bar_w, height: 1 };
        info.push(pl);
        info.push(controls_line(app, &pb, &track, &th));
        f.render_widget(Paragraph::new(info), parts[0]);

        let head = format!("─ Lyrics {}", "─".repeat((parts[1].width as usize).saturating_sub(10)));
        f.render_widget(Paragraph::new(Line::from(Span::styled(head, dim))), parts[1]);
        let lines = lyrics_lines(app, parts[2].height as usize, &th);
        f.render_widget(Paragraph::new(lines), parts[2]);
    }

    if viz_h > 0 {
        let lines = bars_lines(app, viz_rect.width as usize, viz_rect.height as usize, &th);
        f.render_widget(Paragraph::new(lines), viz_rect);
    }
}

fn draw_helpbar(f: &mut Frame, app: &App, area: Rect) {
    let th = app.theme;
    let line = match &app.status {
        Some(s) => Line::from(Span::styled(
            format!(" {}", s.text.replace('\n', " ")),
            Style::default().fg(if s.error { th.error } else { th.accent }),
        )),
        None => {
            let items: [(Action, &str); 10] = [
                (Action::PlayPause, "play/pause"),
                (Action::NextTrack, "next"),
                (Action::PrevTrack, "prev"),
                (Action::Search, "search"),
                (Action::ViewQueue, "queue"),
                (Action::Fullscreen, "fullscreen"),
                (Action::Like, "like"),
                (Action::Devices, "devices"),
                (Action::Help, "help"),
                (Action::Quit, "quit"),
            ];
            let key_style = Style::default().fg(th.accent).add_modifier(Modifier::BOLD);
            let dim = Style::default().fg(th.dim);
            let mut spans: Vec<Span<'static>> = vec![Span::raw(" ")];
            let mut used = 1usize;
            for (action, label) in items {
                let key = app.keymap.first(action);
                let need = key.chars().count() + label.len() + 3;
                if used + need > area.width as usize {
                    break;
                }
                used += need;
                spans.push(Span::styled(key, key_style));
                spans.push(Span::styled(format!(" {label}  "), dim));
            }
            Line::from(spans)
        }
    };
    f.render_widget(Paragraph::new(line), area);
}

// ───────────────────────────── popups ─────────────────────────────

fn draw_help(f: &mut Frame, app: &mut App, area: Rect) {
    let th = app.theme;
    let boxr = centered_pct(area, 70, 90);
    f.render_widget(Clear, boxr);
    let blk = block(app, "Keybindings (Esc to close)".to_string(), true);
    let inner = blk.inner(boxr);
    f.render_widget(blk, boxr);
    let mut lines: Vec<Line> = Vec::new();
    for (_, action, desc) in ACTIONS {
        lines.push(Line::from(vec![
            Span::styled(
                fit(&app.keymap.keys_for(*action), 24),
                Style::default().fg(th.accent).add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
            Span::styled(desc.to_string(), Style::default().fg(th.fg)),
        ]));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        format!("Edit keys, colors and behavior in {}", app.cfg_path.display()),
        Style::default().fg(th.dim),
    )));
    f.render_widget(Paragraph::new(lines), inner);
}

fn draw_devices(f: &mut Frame, app: &mut App, area: Rect) {
    let th = app.theme;
    let boxr = centered(area, 60.min(area.width), 14.min(area.height));
    f.render_widget(Clear, boxr);
    let blk = block(app, "Devices (Enter: switch · Esc: close)".to_string(), true);
    let inner = blk.inner(boxr);
    if app.devices_loading {
        f.render_widget(
            Paragraph::new("Loading devices…").style(Style::default().fg(th.dim)).block(blk),
            boxr,
        );
        return;
    }
    if app.devices.is_empty() {
        f.render_widget(
            Paragraph::new("No devices found.\nStart Spotify on this PC (spotify-launcher, spotifyd…) or on your phone.")
                .style(Style::default().fg(th.dim))
                .wrap(Wrap { trim: false })
                .block(blk),
            boxr,
        );
        return;
    }
    let w = inner.width as usize;
    let items: Vec<ListItem> = app
        .devices
        .iter()
        .map(|d| {
            let mark = if d.is_active { "▶ " } else { "  " };
            let vol = d.volume.map(|v| format!("{v}%")).unwrap_or_default();
            let text = format!("{mark}{} ({}) {vol}", d.name, d.kind.to_lowercase());
            ListItem::new(Line::from(Span::styled(fit(&text, w), Style::default().fg(th.fg))))
        })
        .collect();
    let list = List::new(items).block(blk).highlight_style(sel_style(&th));
    f.render_stateful_widget(list, boxr, &mut app.devices_state);
}

fn draw_player(f: &mut Frame, app: &mut App, area: Rect) {
    let th = app.theme;
    let dim = Style::default().fg(th.dim);
    let acc = Style::default().fg(th.accent).add_modifier(Modifier::BOLD);
    let err = Style::default().fg(th.error);
    let fg = Style::default().fg(th.fg);

    let boxr = centered_pct(area, 85, 85);
    f.render_widget(Clear, boxr);
    let blk = block(app, "Built-in player (librespot) · Esc closes".to_string(), true);
    let inner = blk.inner(boxr);
    f.render_widget(blk, boxr);
    let parts = Layout::vertical([
        Constraint::Length(inner.height.min(15)),
        Constraint::Min(0),
    ])
    .split(inner);

    let backend = if app.cfg.player.backend.trim().is_empty() {
        "default".to_string()
    } else {
        app.cfg.player.backend.clone()
    };
    let mut lines: Vec<Line> = Vec::new();
    match &app.player_state {
        PlayerState::Off => {
            lines.push(Line::from(Span::styled(
                "The built-in player is off ([player] enabled = false in the config).",
                dim,
            )));
        }
        PlayerState::Starting => {
            lines.push(Line::from(Span::styled("Starting librespot…", acc)));
        }
        PlayerState::NeedsLogin => {
            lines.push(Line::from(Span::styled("Sign in to Spotify for the built-in player", acc)));
            lines.push(Line::from(Span::styled(
                "(a one-time approval, separate from spotui's own: librespot signs in as Spotify's desktop app)",
                dim,
            )));
            lines.push(Line::from(""));
            lines.push(Line::from("If your browser did not open, visit:"));
            lines.push(Line::from(Span::styled(
                app.player_login_url.clone().unwrap_or_default(),
                Style::default().fg(th.accent),
            )));
        }
        PlayerState::Connecting => {
            lines.push(Line::from(Span::styled(
                "Signed in. Waiting for the device to appear in Spotify…",
                acc,
            )));
        }
        PlayerState::Ready => {
            lines.push(Line::from(Span::styled(
                format!("Ready: \"{}\" is available as a Spotify device.", app.cfg.player.device_name),
                acc,
            )));
            lines.push(Line::from(Span::styled(
                "Pick a track or press space to play on this computer. Audio stops when you quit spotui.",
                fg,
            )));
        }
        PlayerState::Failed(msg) => {
            lines.push(Line::from(Span::styled("The built-in player is not running", err)));
            lines.push(Line::from(""));
            for l in msg.lines() {
                lines.push(Line::from(Span::styled(l.to_string(), err)));
            }
        }
    }
    if !app.player_last_error.is_empty() && !matches!(app.player_state, PlayerState::Failed(_)) {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!("Last error: {}", app.player_last_error),
            err,
        )));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        format!(
            "device: {} · backend: {} · binary: {}",
            app.cfg.player.device_name, backend, app.cfg.player.binary
        ),
        dim,
    )));
    lines.push(Line::from(Span::styled(
        "o open link · y copy link · r restart · l forget saved login and sign in again",
        dim,
    )));
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), parts[0]);

    let h = parts[1].height as usize;
    let w = parts[1].width as usize;
    if h > 1 {
        let mut log: Vec<Line> = vec![Line::from(Span::styled("Recent librespot output:", dim))];
        let skip = app.player_log.len().saturating_sub(h - 1);
        for l in app.player_log.iter().skip(skip) {
            log.push(Line::from(Span::styled(fit(l, w).trim_end().to_string(), dim)));
        }
        f.render_widget(Paragraph::new(log), parts[1]);
    }
}
