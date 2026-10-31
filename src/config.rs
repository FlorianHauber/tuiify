//! Configuration: TOML file, theme colors and keybindings.

use anyhow::{Context, Result};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::style::Color;
use ratatui::widgets::BorderType;
use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

pub const DEFAULT_CONFIG: &str = r##"# spotui configuration
#
# Location: ~/.config/spotui/config.toml
# Override with `spotui --config <file>` or the SPOTUI_CONFIG environment variable.
# Every option is optional: delete anything you do not want to change.
# Print this file again with: spotui --print-default-config

[general]
# Client ID of your own Spotify app (https://developer.spotify.com/dashboard).
# Leave empty and spotui will ask for it on first start.
client_id = ""
# Add  http://127.0.0.1:<redirect_port>/callback  as a Redirect URI in your Spotify app.
redirect_port = 8888
# UI refresh interval in milliseconds (progress bar, lyrics highlight).
tick_rate_ms = 200
# How often to ask Spotify for the playback state, in ms. Lower = more API calls.
poll_interval_ms = 3000
show_sidebar = true
sidebar_width = 28
show_help_bar = true
volume_step = 5
seek_seconds = 5
mouse = true
# Open the browser automatically during sign-in.
open_browser = true
# Command used to open links. Empty = xdg-open.
browser_command = ""
# If no device is active, start on the device whose name contains this text.
preferred_device = ""
# plain | rounded | double | thick
border_style = "rounded"

[player]
# Built-in player: spotui starts `librespot` itself and shows up as a Spotify Connect device,
# so no other Spotify app has to be open. Needs Spotify Premium and the `librespot` package
# (Arch: sudo pacman -S librespot). Audio stops when you quit spotui.
enabled = true
binary = "librespot"
# Name of the device in Spotify's device list.
device_name = "spotui"
device_type = "computer"
# Audio backend: "pulseaudio" also works on PipeWire. "" = librespot's default.
# List the ones your build has with: librespot --backend ?
backend = "pulseaudio"
# Output device name (backend specific). Empty = default.
device = ""
# 96, 160 or 320
bitrate = 320
initial_volume = 70
normalisation = false
# "oauth"    = sign in through your browser the first time (default)
# "zeroconf" = wait for the official Spotify app on your network to hand over the login
auth = "oauth"
# Extra command-line arguments passed to librespot, e.g. ["--autoplay"]
extra_args = []

[ui]
# Tab bar at the top of the screen.
show_header = true
# Small symbols in the library list. Turn off if your font lacks them.
icons = true
scrollbar = true
# Largest cover (in terminal columns) in full-screen mode.
fullscreen_art_max = 48

[art]
# auto | kitty | halfblock | off
#   auto      = kitty graphics protocol in kitty/ghostty/WezTerm, half-blocks elsewhere
#   halfblock = works in every truecolor terminal
mode = "auto"
# Width of the cover art in terminal columns (height is half of that).
size = 12

[lyrics]
# Synced lyrics are fetched from LRCLIB (https://lrclib.net).
enabled = true
# Shift lyric timing in milliseconds (positive = lyrics appear later).
offset_ms = 0

[visualizer]
# Captures your system audio output (needs `parec`, part of libpulse, works with PipeWire).
# It only "hears" audio playing on THIS computer.
enabled = true
# "@DEFAULT_MONITOR@" = whatever your default output is. List sources: pactl list short sources
source = "@DEFAULT_MONITOR@"
bars = 64
fps = 30
bar_width = 2
bar_gap = 1
gain = 1.0
# 0.0 (instant fall) .. 0.99 (slow fall)
decay = 0.82

[theme]
# Colors: names (red, darkgray, ...), "#rrggbb", 0-255 (ANSI index) or "reset" (terminal default).
background = "reset"
foreground = "reset"
accent = "#1db954"
dim = "darkgray"
selection_bg = "#2b2b2b"
selection_fg = "white"
border = "darkgray"
border_focused = "#1db954"
playing = "#1db954"
error = "#ff5555"
viz_low = "#1db954"
viz_high = "#e91e63"

[keybindings]
# Each action takes a list of keys. Syntax: "j", "G", "ctrl+d", "alt+x", "shift+tab",
# "enter", "esc", "space", "tab", "backspace", "up", "down", "left", "right",
# "pageup", "pagedown", "home", "end", "f1".."f12".
# Only the actions you list here are changed; the rest keep their defaults.
quit = ["ctrl+c"]
up = ["k", "up"]
down = ["j", "down"]
left = ["h", "left"]
right = ["l", "right"]
top = ["g", "home"]
bottom = ["G", "end"]
page_up = ["ctrl+u", "pageup"]
page_down = ["ctrl+d", "pagedown"]
select = ["enter"]
play_context = ["P"]
back = ["backspace", "esc"]
play_pause = ["space"]
next_track = ["n"]
prev_track = ["p"]
volume_up = ["+", "="]
volume_down = ["-", "_"]
seek_forward = [">"]
seek_back = ["<"]
shuffle = ["s"]
repeat = ["r"]
like = ["L"]
fullscreen = ["f"]
add_to_queue = ["a"]
search = ["/"]
view_browse = ["1"]
view_queue = ["q", "2"]
view_lyrics = ["3"]
view_visualizer = ["4"]
devices = ["d"]
goto_album = ["A"]
goto_artist = ["ctrl+a"]
cycle_focus = ["tab"]
refresh = ["R"]
player = ["ctrl+p"]
help = ["?"]
"##;

// ───────────────────────────── config structs ─────────────────────────────

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct Config {
    pub general: General,
    pub player: PlayerCfg,
    pub ui: UiCfg,
    pub art: ArtCfg,
    pub lyrics: LyricsCfg,
    pub visualizer: VizCfg,
    pub theme: ThemeCfg,
    pub keybindings: HashMap<String, Vec<String>>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct General {
    pub client_id: String,
    pub redirect_port: u16,
    pub tick_rate_ms: u64,
    pub poll_interval_ms: u64,
    pub show_sidebar: bool,
    pub sidebar_width: u16,
    pub show_help_bar: bool,
    pub volume_step: u8,
    pub seek_seconds: u64,
    pub mouse: bool,
    pub open_browser: bool,
    pub browser_command: String,
    pub preferred_device: String,
    pub border_style: String,
}

impl Default for General {
    fn default() -> Self {
        General {
            client_id: String::new(),
            redirect_port: 8888,
            tick_rate_ms: 200,
            poll_interval_ms: 3000,
            show_sidebar: true,
            sidebar_width: 28,
            show_help_bar: true,
            volume_step: 5,
            seek_seconds: 5,
            mouse: true,
            open_browser: true,
            browser_command: String::new(),
            preferred_device: String::new(),
            border_style: "rounded".to_string(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct PlayerCfg {
    pub enabled: bool,
    pub binary: String,
    pub device_name: String,
    pub device_type: String,
    pub backend: String,
    pub device: String,
    pub bitrate: u32,
    pub initial_volume: u8,
    pub normalisation: bool,
    pub auth: String,
    pub extra_args: Vec<String>,
}

impl Default for PlayerCfg {
    fn default() -> Self {
        PlayerCfg {
            enabled: true,
            binary: "librespot".to_string(),
            device_name: "spotui".to_string(),
            device_type: "computer".to_string(),
            backend: "pulseaudio".to_string(),
            device: String::new(),
            bitrate: 320,
            initial_volume: 70,
            normalisation: false,
            auth: "oauth".to_string(),
            extra_args: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct UiCfg {
    pub show_header: bool,
    pub icons: bool,
    pub scrollbar: bool,
    pub fullscreen_art_max: u16,
}

impl Default for UiCfg {
    fn default() -> Self {
        UiCfg { show_header: true, icons: true, scrollbar: true, fullscreen_art_max: 48 }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ArtCfg {
    pub mode: String,
    pub size: u16,
}

impl Default for ArtCfg {
    fn default() -> Self {
        ArtCfg { mode: "auto".to_string(), size: 12 }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct LyricsCfg {
    pub enabled: bool,
    pub offset_ms: i64,
}

impl Default for LyricsCfg {
    fn default() -> Self {
        LyricsCfg { enabled: true, offset_ms: 0 }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct VizCfg {
    pub enabled: bool,
    pub source: String,
    pub bars: usize,
    pub fps: u64,
    pub bar_width: u16,
    pub bar_gap: u16,
    pub gain: f32,
    pub decay: f32,
}

impl Default for VizCfg {
    fn default() -> Self {
        VizCfg {
            enabled: true,
            source: "@DEFAULT_MONITOR@".to_string(),
            bars: 64,
            fps: 30,
            bar_width: 2,
            bar_gap: 1,
            gain: 1.0,
            decay: 0.82,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ThemeCfg {
    pub background: String,
    pub foreground: String,
    pub accent: String,
    pub dim: String,
    pub selection_bg: String,
    pub selection_fg: String,
    pub border: String,
    pub border_focused: String,
    pub playing: String,
    pub error: String,
    pub viz_low: String,
    pub viz_high: String,
}

impl Default for ThemeCfg {
    fn default() -> Self {
        ThemeCfg {
            background: "reset".to_string(),
            foreground: "reset".to_string(),
            accent: "#1db954".to_string(),
            dim: "darkgray".to_string(),
            selection_bg: "#2b2b2b".to_string(),
            selection_fg: "white".to_string(),
            border: "darkgray".to_string(),
            border_focused: "#1db954".to_string(),
            playing: "#1db954".to_string(),
            error: "#ff5555".to_string(),
            viz_low: "#1db954".to_string(),
            viz_high: "#e91e63".to_string(),
        }
    }
}

// ───────────────────────────── loading / saving ─────────────────────────────

pub fn config_path(over: Option<PathBuf>) -> PathBuf {
    if let Some(p) = over {
        return p;
    }
    if let Ok(p) = std::env::var("SPOTUI_CONFIG") {
        if !p.is_empty() {
            return PathBuf::from(p);
        }
    }
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("spotui")
        .join("config.toml")
}

/// Updates key bindings that older spotui versions wrote into the config file, so the new
/// defaults (q = queue, f = fullscreen, L = like, ctrl+c = quit) work for existing users.
/// Only lines that still have the exact old default value are touched.
fn migrate(text: &str) -> Option<String> {
    let mut changed = false;
    let mut has_fullscreen = false;
    let mut out: Vec<String> = Vec::new();
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with("fullscreen") && t.contains('=') {
            has_fullscreen = true;
        }
        let repl = match t {
            "quit = [\"q\", \"ctrl+c\"]" => Some("quit = [\"ctrl+c\"]"),
            "like = [\"f\"]" => Some("like = [\"L\"]"),
            "view_queue = [\"2\"]" => Some("view_queue = [\"q\", \"2\"]"),
            _ => None,
        };
        match repl {
            Some(r) => {
                out.push(r.to_string());
                changed = true;
            }
            None => out.push(line.to_string()),
        }
    }
    if !has_fullscreen {
        if let Some(i) = out.iter().position(|l| l.trim() == "[keybindings]") {
            out.insert(i + 1, "fullscreen = [\"f\"]".to_string());
            changed = true;
        }
    }
    if changed {
        Some(out.join("\n") + "\n")
    } else {
        None
    }
}

pub fn load(over: Option<PathBuf>) -> Result<(Config, PathBuf)> {
    let path = config_path(over);
    if !path.exists() {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)
                .with_context(|| format!("cannot create {}", dir.display()))?;
        }
        fs::write(&path, DEFAULT_CONFIG)
            .with_context(|| format!("cannot write {}", path.display()))?;
    }
    let mut text = fs::read_to_string(&path)
        .with_context(|| format!("cannot read {}", path.display()))?;
    if let Some(new_text) = migrate(&text) {
        let bak = path.with_extension("toml.bak");
        if !bak.exists() {
            let _ = fs::write(&bak, &text);
        }
        if fs::write(&path, &new_text).is_ok() {
            text = new_text;
        }
    }
    let cfg: Config = toml::from_str(&text)
        .with_context(|| format!("invalid config file {}", path.display()))?;
    Ok((cfg, path))
}

/// Writes `client_id = "..."` into the config file, keeping everything else intact.
pub fn set_client_id(path: &Path, id: &str) -> Result<()> {
    let text = fs::read_to_string(path).unwrap_or_default();
    let new_line = format!("client_id = \"{id}\"");
    let mut out: Vec<String> = Vec::new();
    let mut replaced = false;
    for line in text.lines() {
        if !replaced && line.trim_start().starts_with("client_id") {
            out.push(new_line.clone());
            replaced = true;
        } else {
            out.push(line.to_string());
        }
    }
    if !replaced {
        let pos = out.iter().position(|l| l.trim() == "[general]");
        match pos {
            Some(i) => out.insert(i + 1, new_line),
            None => {
                out.insert(0, "[general]".to_string());
                out.insert(1, new_line);
            }
        }
    }
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::write(path, out.join("\n") + "\n")?;
    Ok(())
}

// ───────────────────────────── theme ─────────────────────────────

#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub bg: Color,
    pub fg: Color,
    pub accent: Color,
    pub dim: Color,
    pub sel_bg: Color,
    pub sel_fg: Color,
    pub border: Color,
    pub border_focus: Color,
    pub playing: Color,
    pub error: Color,
    pub viz_low: Color,
    pub viz_high: Color,
}

impl Theme {
    pub fn from_cfg(c: &ThemeCfg) -> Theme {
        let p = |s: &str, d: Color| parse_color(s).unwrap_or(d);
        Theme {
            bg: p(&c.background, Color::Reset),
            fg: p(&c.foreground, Color::Reset),
            accent: p(&c.accent, Color::Green),
            dim: p(&c.dim, Color::DarkGray),
            sel_bg: p(&c.selection_bg, Color::DarkGray),
            sel_fg: p(&c.selection_fg, Color::White),
            border: p(&c.border, Color::DarkGray),
            border_focus: p(&c.border_focused, Color::Green),
            playing: p(&c.playing, Color::Green),
            error: p(&c.error, Color::Red),
            viz_low: p(&c.viz_low, Color::Green),
            viz_high: p(&c.viz_high, Color::Magenta),
        }
    }
}

pub fn parse_color(s: &str) -> Option<Color> {
    let s = s.trim().to_lowercase();
    if let Some(hex) = s.strip_prefix('#') {
        if hex.len() == 6 && hex.is_ascii() {
            let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
            let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
            let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
            return Some(Color::Rgb(r, g, b));
        }
        return None;
    }
    if let Ok(n) = s.parse::<u8>() {
        return Some(Color::Indexed(n));
    }
    match s.as_str() {
        "reset" | "default" | "none" | "transparent" => Some(Color::Reset),
        "black" => Some(Color::Black),
        "red" => Some(Color::Red),
        "green" => Some(Color::Green),
        "yellow" => Some(Color::Yellow),
        "blue" => Some(Color::Blue),
        "magenta" => Some(Color::Magenta),
        "cyan" => Some(Color::Cyan),
        "gray" | "grey" => Some(Color::Gray),
        "darkgray" | "darkgrey" => Some(Color::DarkGray),
        "lightred" => Some(Color::LightRed),
        "lightgreen" => Some(Color::LightGreen),
        "lightyellow" => Some(Color::LightYellow),
        "lightblue" => Some(Color::LightBlue),
        "lightmagenta" => Some(Color::LightMagenta),
        "lightcyan" => Some(Color::LightCyan),
        "white" => Some(Color::White),
        _ => None,
    }
}

pub fn lerp_color(a: Color, b: Color, t: f32) -> Color {
    match (a, b) {
        (Color::Rgb(r1, g1, b1), Color::Rgb(r2, g2, b2)) => {
            let l = |x: u8, y: u8| -> u8 {
                (x as f32 + (y as f32 - x as f32) * t).round().clamp(0.0, 255.0) as u8
            };
            Color::Rgb(l(r1, r2), l(g1, g2), l(b1, b2))
        }
        _ => {
            if t < 0.5 {
                a
            } else {
                b
            }
        }
    }
}

pub fn border_type(s: &str) -> BorderType {
    match s.trim().to_lowercase().as_str() {
        "plain" => BorderType::Plain,
        "double" => BorderType::Double,
        "thick" => BorderType::Thick,
        _ => BorderType::Rounded,
    }
}

// ───────────────────────────── actions & keymap ─────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
    Quit,
    Up,
    Down,
    Left,
    Right,
    Top,
    Bottom,
    PageUp,
    PageDown,
    Select,
    PlayContext,
    Back,
    PlayPause,
    NextTrack,
    PrevTrack,
    VolumeUp,
    VolumeDown,
    SeekForward,
    SeekBack,
    Shuffle,
    Repeat,
    Like,
    AddToQueue,
    Search,
    ViewBrowse,
    ViewQueue,
    ViewLyrics,
    ViewVisualizer,
    Devices,
    GotoAlbum,
    GotoArtist,
    CycleFocus,
    Refresh,
    Player,
    Fullscreen,
    Help,
}

/// (config name, action, description shown in the help popup)
pub const ACTIONS: &[(&str, Action, &str)] = &[
    ("quit", Action::Quit, "Quit spotui"),
    ("up", Action::Up, "Move up"),
    ("down", Action::Down, "Move down"),
    ("left", Action::Left, "Go back / focus the sidebar"),
    ("right", Action::Right, "Open selection / focus the list"),
    ("top", Action::Top, "Jump to the first entry"),
    ("bottom", Action::Bottom, "Jump to the last entry"),
    ("page_up", Action::PageUp, "Page up"),
    ("page_down", Action::PageDown, "Page down"),
    ("select", Action::Select, "Play track / open album, artist, playlist"),
    ("play_context", Action::PlayContext, "Play the whole album / playlist / artist"),
    ("back", Action::Back, "Go back one level"),
    ("play_pause", Action::PlayPause, "Play / pause"),
    ("next_track", Action::NextTrack, "Next track"),
    ("prev_track", Action::PrevTrack, "Previous track"),
    ("volume_up", Action::VolumeUp, "Volume up"),
    ("volume_down", Action::VolumeDown, "Volume down"),
    ("seek_forward", Action::SeekForward, "Seek forward"),
    ("seek_back", Action::SeekBack, "Seek backward"),
    ("shuffle", Action::Shuffle, "Toggle shuffle"),
    ("repeat", Action::Repeat, "Cycle repeat: off / context / track"),
    ("like", Action::Like, "Like / unlike the playing track"),
    ("add_to_queue", Action::AddToQueue, "Add selected track to the queue"),
    ("search", Action::Search, "Search the Spotify catalog"),
    ("view_browse", Action::ViewBrowse, "Library view"),
    ("view_queue", Action::ViewQueue, "Queue view (press again to leave)"),
    ("view_lyrics", Action::ViewLyrics, "Synced lyrics view"),
    ("view_visualizer", Action::ViewVisualizer, "Audio visualizer view"),
    ("devices", Action::Devices, "Choose a playback device"),
    ("goto_album", Action::GotoAlbum, "Open the album of the selected/playing track"),
    ("goto_artist", Action::GotoArtist, "Open the artist of the selected/playing track"),
    ("cycle_focus", Action::CycleFocus, "Switch sidebar/list focus (search: change type)"),
    ("refresh", Action::Refresh, "Reload the current list and playlists"),
    ("player", Action::Player, "Built-in player status / sign-in panel"),
    ("fullscreen", Action::Fullscreen, "Full-screen now playing (cover, lyrics, visualizer)"),
    ("help", Action::Help, "Show this help"),
];

pub fn action_from_name(n: &str) -> Option<Action> {
    ACTIONS.iter().find(|(name, _, _)| *name == n).map(|(_, a, _)| *a)
}

const DEFAULT_KEYS: &[(&str, &[&str])] = &[
    ("quit", &["ctrl+c"]),
    ("up", &["k", "up"]),
    ("down", &["j", "down"]),
    ("left", &["h", "left"]),
    ("right", &["l", "right"]),
    ("top", &["g", "home"]),
    ("bottom", &["G", "end"]),
    ("page_up", &["ctrl+u", "pageup"]),
    ("page_down", &["ctrl+d", "pagedown"]),
    ("select", &["enter"]),
    ("play_context", &["P"]),
    ("back", &["backspace", "esc"]),
    ("play_pause", &["space"]),
    ("next_track", &["n"]),
    ("prev_track", &["p"]),
    ("volume_up", &["+", "="]),
    ("volume_down", &["-", "_"]),
    ("seek_forward", &[">"]),
    ("seek_back", &["<"]),
    ("shuffle", &["s"]),
    ("repeat", &["r"]),
    ("like", &["L"]),
    ("fullscreen", &["f"]),
    ("add_to_queue", &["a"]),
    ("search", &["/"]),
    ("view_browse", &["1"]),
    ("view_queue", &["q", "2"]),
    ("view_lyrics", &["3"]),
    ("view_visualizer", &["4"]),
    ("devices", &["d"]),
    ("goto_album", &["A"]),
    ("goto_artist", &["ctrl+a"]),
    ("cycle_focus", &["tab"]),
    ("refresh", &["R"]),
    ("player", &["ctrl+p"]),
    ("help", &["?"]),
];

pub fn action_name(a: Action) -> &'static str {
    ACTIONS
        .iter()
        .find(|(_, act, _)| *act == a)
        .map(|(n, _, _)| *n)
        .unwrap_or("?")
}

pub struct KeyMap {
    map: HashMap<(KeyCode, KeyModifiers), Action>,
    bindings: Vec<(Action, Vec<String>)>,
}

fn normalize(code: KeyCode, mods: KeyModifiers) -> (KeyCode, KeyModifiers) {
    match code {
        KeyCode::Char(c) => {
            let m = mods - KeyModifiers::SHIFT;
            let c = if m.contains(KeyModifiers::CONTROL) {
                c.to_ascii_lowercase()
            } else {
                c
            };
            (KeyCode::Char(c), m)
        }
        KeyCode::BackTab => (code, mods - KeyModifiers::SHIFT),
        _ => (code, mods),
    }
}

pub fn parse_key(s: &str) -> Option<(KeyCode, KeyModifiers)> {
    let mut rest = s.trim();
    let mut mods = KeyModifiers::NONE;
    loop {
        let low = rest.to_ascii_lowercase();
        if low.starts_with("ctrl+") && rest.len() > 5 {
            mods |= KeyModifiers::CONTROL;
            rest = &rest[5..];
        } else if low.starts_with("alt+") && rest.len() > 4 {
            mods |= KeyModifiers::ALT;
            rest = &rest[4..];
        } else if low.starts_with("shift+") && rest.len() > 6 {
            mods |= KeyModifiers::SHIFT;
            rest = &rest[6..];
        } else {
            break;
        }
    }
    let low = rest.to_ascii_lowercase();
    let mut code = match low.as_str() {
        "enter" | "return" => KeyCode::Enter,
        "esc" | "escape" => KeyCode::Esc,
        "space" => KeyCode::Char(' '),
        "tab" => KeyCode::Tab,
        "backtab" => KeyCode::BackTab,
        "backspace" => KeyCode::Backspace,
        "delete" | "del" => KeyCode::Delete,
        "insert" => KeyCode::Insert,
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        "pageup" => KeyCode::PageUp,
        "pagedown" => KeyCode::PageDown,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        other => {
            if other.len() > 1 && other.starts_with('f') {
                if let Ok(n) = other[1..].parse::<u8>() {
                    KeyCode::F(n)
                } else {
                    return None;
                }
            } else {
                let mut it = rest.chars();
                match (it.next(), it.next()) {
                    (Some(c), None) => KeyCode::Char(c),
                    _ => return None,
                }
            }
        }
    };
    if let KeyCode::Char(c) = code {
        if mods.contains(KeyModifiers::SHIFT) {
            code = KeyCode::Char(c.to_ascii_uppercase());
        }
    }
    Some(normalize(code, mods))
}

impl KeyMap {
    /// Builds the keymap from defaults plus user overrides. Returns warnings for bad entries.
    pub fn build(user: &HashMap<String, Vec<String>>) -> (KeyMap, Vec<String>) {
        let mut warnings = Vec::new();
        let mut table: Vec<(Action, Vec<String>)> = Vec::new();
        for (name, keys) in DEFAULT_KEYS {
            if let Some(action) = action_from_name(name) {
                let list: Vec<String> = match user.get(*name) {
                    Some(u) => u.clone(),
                    None => keys.iter().map(|s| s.to_string()).collect(),
                };
                table.push((action, list));
            }
        }
        for name in user.keys() {
            if action_from_name(name).is_none() {
                warnings.push(format!("unknown action '{name}' in [keybindings]"));
            }
        }
        let mut map = HashMap::new();
        for (action, keys) in &table {
            for k in keys {
                match parse_key(k) {
                    Some(kc) => {
                        if let Some(prev) = map.insert(kc, *action) {
                            if prev != *action {
                                warnings.push(format!(
                                    "key '{k}' is bound to both {} and {} (using {})",
                                    action_name(prev),
                                    action_name(*action),
                                    action_name(*action)
                                ));
                            }
                        }
                    }
                    None => warnings.push(format!("cannot parse key '{k}'")),
                }
            }
        }
        (KeyMap { map, bindings: table }, warnings)
    }

    pub fn lookup(&self, ev: &KeyEvent) -> Option<Action> {
        let k = normalize(ev.code, ev.modifiers);
        self.map.get(&k).copied()
    }

    pub fn keys_for(&self, a: Action) -> String {
        self.bindings
            .iter()
            .find(|(act, _)| *act == a)
            .map(|(_, keys)| keys.join(" / "))
            .unwrap_or_default()
    }

    pub fn first(&self, a: Action) -> String {
        self.bindings
            .iter()
            .find(|(act, _)| *act == a)
            .and_then(|(_, keys)| keys.first().cloned())
            .unwrap_or_else(|| "-".to_string())
    }
}
