mod api;
mod app;
mod art;
mod auth;
mod config;
mod lyrics;
mod models;
mod player;
mod ui;
mod viz;

use anyhow::Result;
use app::{App, Msg};
use art::ArtMode;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::Terminal;
use std::io::{self, Write};
use std::path::PathBuf;
use std::time::Duration;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};

const HELP: &str = "spotui - a fast, customizable Spotify TUI

USAGE:
    spotui [OPTIONS]

OPTIONS:
    -c, --config <FILE>      Use a different config file
        --config-path        Print the config file location and exit
        --print-default-config
                             Print the default config and exit
        --logout             Delete the stored Spotify login and exit
    -h, --help               Show this help
    -V, --version            Show the version

Config:  ~/.config/spotui/config.toml   (created on first start)
Token:   ~/.local/share/spotui/token.json
Press ? inside spotui to see all keybindings.";

type Term = Terminal<CrosstermBackend<io::Stdout>>;

fn setup_terminal(mouse: bool) -> Result<Term> {
    enable_raw_mode()?;
    let mut out = io::stdout();
    execute!(out, EnterAlternateScreen, EnableBracketedPaste)?;
    if mouse {
        execute!(out, EnableMouseCapture)?;
    }
    Ok(Terminal::new(CrosstermBackend::new(out))?)
}

fn restore_terminal() {
    let mut out = io::stdout();
    let _ = execute!(out, DisableMouseCapture, DisableBracketedPaste, LeaveAlternateScreen);
    let _ = disable_raw_mode();
    let _ = out.flush();
}

fn spawn_input_thread(tx: UnboundedSender<Msg>) {
    std::thread::spawn(move || loop {
        match event::poll(Duration::from_millis(100)) {
            Ok(true) => match event::read() {
                Ok(ev) => {
                    if tx.send(Msg::Input(ev)).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            },
            Ok(false) => {
                if tx.is_closed() {
                    break;
                }
            }
            Err(_) => break,
        }
    });
}

fn spawn_ticker(tx: UnboundedSender<Msg>, ms: u64) {
    tokio::spawn(async move {
        let mut iv = tokio::time::interval(Duration::from_millis(ms.max(50)));
        loop {
            iv.tick().await;
            if tx.send(Msg::Tick).is_err() {
                break;
            }
        }
    });
}

/// Keeps the kitty-protocol cover image in sync with what the last frame wants.
fn sync_kitty(terminal: &mut Term, app: &mut App) -> Result<()> {
    if app.art_mode != ArtMode::Kitty {
        return Ok(());
    }
    let desired = app.kitty_desired();
    if app.kitty_force_clear || desired != app.kitty_shown {
        let out = terminal.backend_mut();
        if app.kitty_shown.is_some() || app.kitty_force_clear {
            art::kitty_clear(&mut *out)?;
        }
        if let (Some((_, rect)), Some(a)) = (desired, app.kitty_art()) {
            art::kitty_place(&mut *out, a, rect.x, rect.y)?;
        }
        app.kitty_shown = desired;
        app.kitty_force_clear = false;
    }
    Ok(())
}

async fn run(
    terminal: &mut Term,
    app: &mut App,
    rx: &mut UnboundedReceiver<Msg>,
) -> Result<()> {
    loop {
        terminal.draw(|f| ui::draw(f, app))?;
        app.after_draw();
        sync_kitty(terminal, app)?;
        let msg = match rx.recv().await {
            Some(m) => m,
            None => return Ok(()),
        };
        app.update(msg);
        while let Ok(m) = rx.try_recv() {
            app.update(m);
        }
        if app.should_quit {
            return Ok(());
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let mut config_override: Option<PathBuf> = None;
    let mut logout = false;
    while let Some(a) = args.next() {
        match a.as_str() {
            "-h" | "--help" => {
                println!("{HELP}");
                return Ok(());
            }
            "-V" | "--version" => {
                println!("spotui {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            "--print-default-config" => {
                print!("{}", config::DEFAULT_CONFIG);
                return Ok(());
            }
            "--config-path" => {
                println!("{}", config::config_path(config_override.clone()).display());
                return Ok(());
            }
            "-c" | "--config" => match args.next() {
                Some(p) => config_override = Some(PathBuf::from(p)),
                None => {
                    eprintln!("--config needs a file path");
                    std::process::exit(2);
                }
            },
            "--logout" => logout = true,
            other => {
                eprintln!("unknown argument: {other}\n\n{HELP}");
                std::process::exit(2);
            }
        }
    }
    if logout {
        auth::delete_token();
        println!("Signed out: stored token removed.");
        return Ok(());
    }

    let (cfg, cfg_path) = match config::load(config_override) {
        Ok(x) => x,
        Err(e) => {
            eprintln!("spotui: {e:#}");
            std::process::exit(1);
        }
    };

    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal();
        default_hook(info);
    }));

    let (tx, mut rx) = mpsc::unbounded_channel::<Msg>();
    let mut terminal = setup_terminal(cfg.general.mouse)?;
    let tick = cfg.general.tick_rate_ms;
    let mut app = App::new(cfg, cfg_path, tx.clone());
    spawn_input_thread(tx.clone());
    spawn_ticker(tx.clone(), tick);
    app.start();

    let res = run(&mut terminal, &mut app, &mut rx).await;

    app.shutdown();
    if app.kitty_shown.is_some() {
        let _ = art::kitty_clear(terminal.backend_mut());
    }
    restore_terminal();
    res
}
