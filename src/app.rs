//! Application state and logic. All state lives in `App` and is only mutated from the
//! main loop; background tasks talk to it through `Msg`.

use crate::api::{self, Api};
use crate::art::{self, Art, ArtMode};
use crate::auth::{self, Token};
use crate::config::{self, Action, Config, KeyMap, Theme};
use crate::lyrics::{self, Lyrics};
use crate::models::*;
use crate::player::{self, PlayerProc};
use crate::viz::Viz;
use ratatui::crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::Rect;
use ratatui::widgets::{BorderType, ListState};
use serde_json::json;
use std::collections::VecDeque;
use std::future::Future;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use tokio::sync::mpsc::UnboundedSender;

// ───────────────────────────── messages ─────────────────────────────

pub enum Msg {
    Input(Event),
    Tick,
    LoggedIn(Token),
    LoginFailed(String),
    AuthExpired,
    Playback(Option<Playback>),
    PollFailed(String),
    PollSoon,
    Playlists(Vec<Playlist>),
    Page { id: u64, page: Page },
    PageFailed { id: u64, error: String, auth: bool, status: u16 },
    Queue(Result<(Option<Track>, Vec<Track>), String>),
    Devices(Result<Vec<Device>, String>),
    Lyrics { uri: String, result: Result<Option<Lyrics>, String> },
    Cover { url: String, bytes: Result<Arc<Vec<u8>>, String> },
    Art { url: String, art: Result<Art, String> },
    ArtFull { url: String, dims: (u16, u16), art: Result<Art, String> },
    Liked { id: String, liked: bool },
    Viz(Vec<f32>),
    VizError(String),
    PlayerLine(u64, String),
    PlayerExit(u64, Option<i32>),
    PlayerProbe(Result<Option<Device>, String>),
    Error(String),
}

fn report(tx: &UnboundedSender<Msg>, e: &anyhow::Error) {
    if api::is_auth_error(e) {
        let _ = tx.send(Msg::AuthExpired);
    } else {
        let _ = tx.send(Msg::Error(api::describe_error(e)));
    }
}

// ───────────────────────────── state types ─────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Screen {
    Login,
    Main,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum View {
    Browse,
    Search,
    Queue,
    Lyrics,
    Visualizer,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Focus {
    Sidebar,
    Main,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Popup {
    None,
    Help,
    Devices,
    Player,
}

#[derive(Clone, PartialEq, Debug)]
pub enum PlayerState {
    Off,
    Starting,
    NeedsLogin,
    Connecting,
    Ready,
    Failed(String),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LoginPhase {
    NeedClientId,
    Waiting,
    Failed,
}

pub struct LoginState {
    pub phase: LoginPhase,
    pub url: String,
    pub input: String,
    pub error: Option<String>,
}

#[derive(Clone, Debug)]
pub enum SidebarEntry {
    Liked,
    Recent,
    Top,
    Albums,
    Artists,
    Playlist(Playlist),
}

impl SidebarEntry {
    pub fn label(&self) -> String {
        match self {
            SidebarEntry::Liked => "Liked Songs".to_string(),
            SidebarEntry::Recent => "Recently Played".to_string(),
            SidebarEntry::Top => "Top Tracks".to_string(),
            SidebarEntry::Albums => "Saved Albums".to_string(),
            SidebarEntry::Artists => "Followed Artists".to_string(),
            SidebarEntry::Playlist(p) => p.name.clone(),
        }
    }
}

fn fixed_sidebar() -> Vec<SidebarEntry> {
    vec![
        SidebarEntry::Liked,
        SidebarEntry::Recent,
        SidebarEntry::Top,
        SidebarEntry::Albums,
        SidebarEntry::Artists,
    ]
}

pub struct Content {
    pub id: u64,
    pub title: String,
    pub source: Source,
    pub items: Vec<Item>,
    pub state: ListState,
    pub next: PageNext,
    pub total: u32,
    pub loading: bool,
    pub error: Option<String>,
    pub context_uri: Option<String>,
}

impl Content {
    pub fn new(id: u64, title: String, source: Source) -> Content {
        let context_uri = match &source {
            Source::Playlist(i) => Some(format!("spotify:playlist:{i}")),
            Source::Album(i) => Some(format!("spotify:album:{i}")),
            _ => None,
        };
        Content {
            id,
            title,
            source,
            items: Vec::new(),
            state: ListState::default(),
            next: PageNext::None,
            total: 0,
            loading: true,
            error: None,
            context_uri,
        }
    }
}

pub struct SearchState {
    pub input: String,
    pub query: String,
    pub typing: bool,
    pub kind: SearchKind,
}

#[derive(Default)]
pub struct QueueState {
    pub current: Option<Track>,
    pub items: Vec<Track>,
    pub state: ListState,
    pub loading: bool,
    pub error: Option<String>,
}

#[derive(Default, PartialEq, Clone)]
pub enum LyricsStatus {
    #[default]
    Idle,
    Loading,
    Found,
    NotFound,
    Failed(String),
}

#[derive(Default)]
pub struct LyricsState {
    pub uri: String,
    pub status: LyricsStatus,
    pub lyrics: Option<Lyrics>,
    pub scroll: usize,
}

pub struct Status {
    pub text: String,
    pub error: bool,
    pub until: Instant,
}

/// Screen rectangles recorded by the last draw; used for mouse hit-testing and kitty art.
#[derive(Clone, Copy, Default)]
pub struct Rects {
    pub sidebar: Rect,
    pub list: Rect,
    pub progress: Rect,
    pub art: Option<Rect>,
}

fn hit(r: Rect, x: u16, y: u16) -> bool {
    r.width > 0 && r.height > 0 && x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height
}

fn move_list(state: &mut ListState, len: usize, delta: i32) {
    if len == 0 {
        return;
    }
    let cur = state.selected().unwrap_or(0) as i64;
    let n = (cur + delta as i64).clamp(0, len as i64 - 1) as usize;
    state.select(Some(n));
}

/// Basic single-line text editing. Returns true if the key was consumed.
fn edit_text(buf: &mut String, key: &KeyEvent) -> bool {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    match key.code {
        KeyCode::Char('u') if ctrl => {
            buf.clear();
            true
        }
        KeyCode::Char('w') if ctrl => {
            let trimmed = buf.trim_end().len();
            buf.truncate(trimmed);
            while let Some(c) = buf.chars().last() {
                if c.is_whitespace() {
                    break;
                }
                buf.pop();
            }
            true
        }
        KeyCode::Char(c) if !ctrl && !alt => {
            buf.push(c);
            true
        }
        KeyCode::Backspace => {
            buf.pop();
            true
        }
        _ => false,
    }
}

pub fn open_url(browser_cmd: &str, url: &str) {
    let cmd = if browser_cmd.trim().is_empty() { "xdg-open" } else { browser_cmd.trim() };
    let mut parts = cmd.split_whitespace();
    let bin = match parts.next() {
        Some(b) => b,
        None => return,
    };
    let _ = Command::new(bin)
        .args(parts)
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

fn copy_to_clipboard(text: &str) -> bool {
    let candidates: Vec<(&str, Vec<&str>)> = vec![
        ("wl-copy", vec![]),
        ("xclip", vec!["-selection", "clipboard"]),
        ("xsel", vec!["--clipboard", "--input"]),
    ];
    for (cmd, args) in candidates {
        if let Ok(mut child) = Command::new(cmd)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(text.as_bytes());
            }
            return true;
        }
    }
    false
}

// ───────────────────────────── App ─────────────────────────────

pub struct App {
    pub cfg: Config,
    pub cfg_path: PathBuf,
    pub theme: Theme,
    pub keymap: KeyMap,
    pub border_type: BorderType,
    pub tx: UnboundedSender<Msg>,
    pub http: reqwest::Client,
    pub api: Option<Api>,

    pub screen: Screen,
    pub login: LoginState,
    login_task: Option<tokio::task::JoinHandle<()>>,

    pub view: View,
    pub focus: Focus,
    pub popup: Popup,

    pub sidebar: Vec<SidebarEntry>,
    pub sidebar_state: ListState,
    pub browse: Vec<Content>,
    pub search_stack: Vec<Content>,
    pub search: SearchState,
    pub queue: QueueState,

    pub playback: Option<Playback>,
    pub art: Option<Art>,
    art_url: Option<String>,
    pub art_mode: ArtMode,
    pub art_full: Option<Art>,
    cover: Option<(String, Arc<Vec<u8>>)>,
    full_req: Option<(String, (u16, u16))>,
    /// Cover size (cols, rows) the fullscreen layout would like; set while drawing.
    pub full_want: Option<(u16, u16)>,
    pub fullscreen: bool,
    pub tick: u64,
    pub kitty_shown: Option<(u32, Rect)>,
    pub kitty_force_clear: bool,
    pub lyrics: LyricsState,
    viz: Option<Viz>,
    pub bars: Vec<f32>,
    pub viz_error: Option<String>,
    pub devices: Vec<Device>,
    pub devices_state: ListState,
    pub devices_loading: bool,
    pub liked: Option<(String, bool)>,

    player: Option<PlayerProc>,
    pub player_state: PlayerState,
    player_gen: u64,
    player_since: Instant,
    player_used_cache: bool,
    player_reset_done: bool,
    player_restarts: u32,
    pub player_login_url: Option<String>,
    pub player_last_error: String,
    pub player_log: VecDeque<String>,
    probe_in_flight: bool,
    next_probe: Instant,

    pub status: Option<Status>,
    pub rects: Rects,
    next_id: u64,
    poll_in_flight: bool,
    next_poll: Instant,
    last_click: Option<(Instant, u8, usize)>,
    pub should_quit: bool,
}

impl App {
    pub fn new(cfg: Config, cfg_path: PathBuf, tx: UnboundedSender<Msg>) -> App {
        let theme = Theme::from_cfg(&cfg.theme);
        let (keymap, warnings) = KeyMap::build(&cfg.keybindings);
        let border_type = config::border_type(&cfg.general.border_style);
        let art_mode = art::detect(&cfg.art.mode);
        let http = reqwest::Client::builder()
            .user_agent(concat!("spotui/", env!("CARGO_PKG_VERSION")))
            .timeout(Duration::from_secs(20))
            .build()
            .expect("failed to build the HTTP client");
        let mut sidebar_state = ListState::default();
        sidebar_state.select(Some(0));
        let mut app = App {
            cfg,
            cfg_path,
            theme,
            keymap,
            border_type,
            tx,
            http,
            api: None,
            screen: Screen::Login,
            login: LoginState {
                phase: LoginPhase::NeedClientId,
                url: String::new(),
                input: String::new(),
                error: None,
            },
            login_task: None,
            view: View::Browse,
            focus: Focus::Main,
            popup: Popup::None,
            sidebar: fixed_sidebar(),
            sidebar_state,
            browse: Vec::new(),
            search_stack: Vec::new(),
            search: SearchState {
                input: String::new(),
                query: String::new(),
                typing: false,
                kind: SearchKind::Track,
            },
            queue: QueueState::default(),
            playback: None,
            art: None,
            art_url: None,
            art_mode,
            art_full: None,
            cover: None,
            full_req: None,
            full_want: None,
            fullscreen: false,
            tick: 0,
            kitty_shown: None,
            kitty_force_clear: false,
            lyrics: LyricsState::default(),
            viz: None,
            bars: Vec::new(),
            viz_error: None,
            devices: Vec::new(),
            devices_state: ListState::default(),
            devices_loading: false,
            liked: None,
            player: None,
            player_state: PlayerState::Off,
            player_gen: 0,
            player_since: Instant::now(),
            player_used_cache: false,
            player_reset_done: false,
            player_restarts: 0,
            player_login_url: None,
            player_last_error: String::new(),
            player_log: VecDeque::new(),
            probe_in_flight: false,
            next_probe: Instant::now(),
            status: None,
            rects: Rects::default(),
            next_id: 0,
            poll_in_flight: false,
            next_poll: Instant::now(),
            last_click: None,
            should_quit: false,
        };
        if !warnings.is_empty() {
            app.set_status(format!("config: {}", warnings.join("; ")), true);
        }
        app
    }

    /// Called once after the terminal is ready.
    pub fn start(&mut self) {
        let id = self.cfg.general.client_id.trim().to_string();
        if let Some(tok) = auth::load_token() {
            if id.is_empty() || tok.client_id == id {
                if id.is_empty() {
                    self.cfg.general.client_id = tok.client_id.clone();
                }
                self.on_authenticated(tok);
                return;
            }
        }
        self.enter_login();
    }

    pub fn shutdown(&mut self) {
        self.stop_viz();
        self.stop_player();
    }

    // ───────────────────────────── small helpers ─────────────────────────────

    fn new_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    pub fn set_status(&mut self, text: impl Into<String>, error: bool) {
        let secs = if error { 7 } else { 3 };
        self.status = Some(Status {
            text: text.into(),
            error,
            until: Instant::now() + Duration::from_secs(secs),
        });
    }

    pub fn art_dims(&self) -> (u16, u16) {
        let cols = self.cfg.art.size.clamp(6, 60);
        let rows = (cols / 2).max(3);
        (cols, rows)
    }

    pub fn current_track(&self) -> Option<&Track> {
        self.playback.as_ref().and_then(|p| p.track.as_ref())
    }

    pub fn progress_ms(&self) -> u64 {
        match &self.playback {
            Some(p) => {
                let mut ms = p.progress_ms;
                if p.is_playing {
                    ms += p.fetched.elapsed().as_millis() as u64;
                }
                if let Some(t) = &p.track {
                    if t.duration_ms > 0 {
                        ms = ms.min(t.duration_ms);
                    }
                }
                ms
            }
            None => 0,
        }
    }

    pub fn kitty_desired(&self) -> Option<(u32, Rect)> {
        if self.art_mode != ArtMode::Kitty || self.popup != Popup::None {
            return None;
        }
        match (self.kitty_art(), self.rects.art) {
            (Some(a), Some(r)) => Some((a.id, r)),
            _ => None,
        }
    }

    /// The cover that is currently shown with the kitty protocol.
    pub fn kitty_art(&self) -> Option<&Art> {
        if self.fullscreen {
            self.art_full.as_ref()
        } else {
            self.art.as_ref()
        }
    }

    pub fn spinner(&self) -> &'static str {
        const FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
        FRAMES[(self.tick % 10) as usize]
    }

    fn sidebar_visible(&self) -> bool {
        self.rects.sidebar.width > 0
    }

    fn sidebar_focused(&self) -> bool {
        self.focus == Focus::Sidebar && self.sidebar_visible()
    }

    fn cur_stack(&self) -> &Vec<Content> {
        if self.view == View::Search {
            &self.search_stack
        } else {
            &self.browse
        }
    }

    fn selected_item(&self) -> Option<(Item, usize)> {
        if self.fullscreen {
            return None;
        }
        let c = self.cur_stack().last()?;
        let i = c.state.selected()?;
        c.items.get(i).cloned().map(|it| (it, i))
    }

    fn find_content_mut(&mut self, id: u64) -> Option<&mut Content> {
        self.browse
            .iter_mut()
            .chain(self.search_stack.iter_mut())
            .find(|c| c.id == id)
    }

    /// Runs a player/library command in the background; refreshes playback afterwards.
    fn cmd<F, Fut>(&self, f: F)
    where
        F: FnOnce(Api) -> Fut + Send + 'static,
        Fut: Future<Output = anyhow::Result<()>> + Send + 'static,
    {
        let api = match self.api.clone() {
            Some(a) => a,
            None => return,
        };
        let tx = self.tx.clone();
        tokio::spawn(async move {
            match f(api).await {
                Ok(()) => {
                    let _ = tx.send(Msg::PollSoon);
                }
                Err(e) => report(&tx, &e),
            }
        });
    }

    // ───────────────────────────── login ─────────────────────────────

    fn enter_login(&mut self) {
        self.screen = Screen::Login;
        if self.cfg.general.client_id.trim().is_empty() {
            self.login.phase = LoginPhase::NeedClientId;
        } else {
            self.start_login();
        }
    }

    fn start_login(&mut self) {
        if let Some(h) = self.login_task.take() {
            h.abort();
        }
        let client_id = self.cfg.general.client_id.trim().to_string();
        let port = self.cfg.general.redirect_port;
        let session = auth::new_session(&client_id, port);
        self.login.url = session.url.clone();
        self.login.phase = LoginPhase::Waiting;
        self.login.error = None;
        if self.cfg.general.open_browser {
            open_url(&self.cfg.general.browser_command, &session.url);
        }
        let tx = self.tx.clone();
        let http = self.http.clone();
        self.login_task = Some(tokio::spawn(async move {
            match auth::login(&http, &client_id, session, port).await {
                Ok(tok) => {
                    let _ = auth::save_token(&tok);
                    let _ = tx.send(Msg::LoggedIn(tok));
                }
                Err(e) => {
                    let _ = tx.send(Msg::LoginFailed(format!("{e:#}")));
                }
            }
        }));
    }

    fn submit_client_id(&mut self) {
        let id = self.login.input.trim().to_string();
        if id.len() < 16 || !id.chars().all(|c| c.is_ascii_alphanumeric()) {
            self.login.error = Some("That does not look like a Spotify Client ID (32 letters/digits).".to_string());
            return;
        }
        if let Err(e) = config::set_client_id(&self.cfg_path, &id) {
            self.login.error = Some(format!("Could not save to the config file: {e:#}"));
            return;
        }
        self.cfg.general.client_id = id;
        self.login.error = None;
        self.start_login();
    }

    fn on_authenticated(&mut self, tok: Token) {
        // Prefer the built-in player's device when something has to be woken up.
        let preferred = if !self.cfg.general.preferred_device.trim().is_empty() {
            self.cfg.general.preferred_device.clone()
        } else if self.cfg.player.enabled {
            self.cfg.player.device_name.clone()
        } else {
            String::new()
        };
        let api = Api::new(self.http.clone(), tok, preferred);
        self.api = Some(api);
        self.screen = Screen::Main;
        self.load_playlists();
        self.open_sidebar_entry(0);
        self.poll_now();
        self.start_player();
    }

    fn auth_expired(&mut self) {
        auth::delete_token();
        self.api = None;
        self.stop_viz();
        self.stop_player();
        self.player_state = PlayerState::Off;
        self.fullscreen = false;
        self.popup = Popup::None;
        self.playback = None;
        self.login.error = Some("Your Spotify session expired. Please sign in again.".to_string());
        self.enter_login();
    }

    // ───────────────────────────── loading data ─────────────────────────────

    fn load_playlists(&self) {
        let api = match self.api.clone() {
            Some(a) => a,
            None => return,
        };
        let tx = self.tx.clone();
        tokio::spawn(async move {
            match api.my_playlists().await {
                Ok(p) => {
                    let _ = tx.send(Msg::Playlists(p));
                }
                Err(e) => report(&tx, &e),
            }
        });
    }

    fn spawn_page(&self, id: u64, source: Source, next: PageNext) {
        let api = match self.api.clone() {
            Some(a) => a,
            None => return,
        };
        let tx = self.tx.clone();
        tokio::spawn(async move {
            match api.fetch_page(&source, &next).await {
                Ok(page) => {
                    let _ = tx.send(Msg::Page { id, page });
                }
                Err(e) => {
                    let _ = tx.send(Msg::PageFailed {
                        id,
                        error: api::describe_error(&e),
                        auth: api::is_auth_error(&e),
                        status: api::api_error(&e).map(|a| a.status).unwrap_or(0),
                    });
                }
            }
        });
    }

    fn poll_now(&mut self) {
        let api = match self.api.clone() {
            Some(a) => a,
            None => return,
        };
        self.poll_in_flight = true;
        self.next_poll =
            Instant::now() + Duration::from_millis(self.cfg.general.poll_interval_ms.max(1000));
        let tx = self.tx.clone();
        tokio::spawn(async move {
            match api.playback().await {
                Ok(pb) => {
                    let _ = tx.send(Msg::Playback(pb));
                }
                Err(e) => {
                    if api::is_auth_error(&e) {
                        let _ = tx.send(Msg::AuthExpired);
                    } else {
                        let _ = tx.send(Msg::PollFailed(api::describe_error(&e)));
                    }
                }
            }
        });
    }

    fn open_sidebar_entry(&mut self, i: usize) {
        let entry = match self.sidebar.get(i) {
            Some(e) => e.clone(),
            None => return,
        };
        self.set_view(View::Browse);
        let (title, source, ctx): (String, Source, Option<String>) = match entry {
            SidebarEntry::Liked => ("Liked Songs".to_string(), Source::Liked, None),
            SidebarEntry::Recent => ("Recently Played".to_string(), Source::Recent, None),
            SidebarEntry::Top => ("Top Tracks".to_string(), Source::TopTracks, None),
            SidebarEntry::Albums => ("Saved Albums".to_string(), Source::SavedAlbums, None),
            SidebarEntry::Artists => ("Followed Artists".to_string(), Source::FollowedArtists, None),
            SidebarEntry::Playlist(p) => {
                (p.name.clone(), Source::Playlist(p.id.clone()), Some(p.uri.clone()))
            }
        };
        let id = self.new_id();
        let mut c = Content::new(id, title, source.clone());
        if ctx.is_some() {
            c.context_uri = ctx;
        }
        self.browse.clear();
        self.browse.push(c);
        self.spawn_page(id, source, PageNext::None);
    }

    fn open_child(&mut self, title: String, source: Source) {
        let id = self.new_id();
        let c = Content::new(id, title, source.clone());
        if self.view == View::Search {
            self.search_stack.push(c);
        } else {
            self.browse.push(c);
        }
        self.spawn_page(id, source, PageNext::None);
    }

    fn after_move(&mut self) {
        if !matches!(self.view, View::Browse | View::Search) || self.focus != Focus::Main {
            return;
        }
        let stack = if self.view == View::Search {
            &mut self.search_stack
        } else {
            &mut self.browse
        };
        let c = match stack.last_mut() {
            Some(c) => c,
            None => return,
        };
        if c.loading || c.next == PageNext::None || c.error.is_some() {
            return;
        }
        let sel = c.state.selected().unwrap_or(0);
        if sel + 15 >= c.items.len() {
            c.loading = true;
            let (id, source, next) = (c.id, c.source.clone(), c.next.clone());
            self.spawn_page(id, source, next);
        }
    }

    fn load_queue(&mut self) {
        let api = match self.api.clone() {
            Some(a) => a,
            None => return,
        };
        self.queue.loading = true;
        self.queue.error = None;
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let r = api.queue().await.map_err(|e| api::describe_error(&e));
            let _ = tx.send(Msg::Queue(r));
        });
    }

    fn ensure_lyrics(&mut self) {
        if !self.cfg.lyrics.enabled {
            return;
        }
        let track = match self.current_track() {
            Some(t) => t.clone(),
            None => return,
        };
        if self.lyrics.uri == track.uri {
            return;
        }
        self.lyrics = LyricsState {
            uri: track.uri.clone(),
            status: LyricsStatus::Loading,
            lyrics: None,
            scroll: 0,
        };
        let http = self.http.clone();
        let tx = self.tx.clone();
        let uri = track.uri.clone();
        tokio::spawn(async move {
            let r = lyrics::fetch(&http, &track).await.map_err(|e| format!("{e:#}"));
            let _ = tx.send(Msg::Lyrics { uri, result: r });
        });
    }

    fn ensure_art(&mut self) {
        if self.art_mode == ArtMode::Off {
            return;
        }
        let url = self.current_track().and_then(|t| t.image_url.clone());
        if url == self.art_url {
            return;
        }
        self.art_url = url.clone();
        let url = match url {
            Some(u) => u,
            None => {
                self.art = None;
                self.art_full = None;
                self.cover = None;
                self.full_req = None;
                return;
            }
        };
        let http = self.http.clone();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let r = art::download(&http, &url).await.map_err(|e| format!("{e:#}"));
            let _ = tx.send(Msg::Cover { url, bytes: r });
        });
    }

    fn render_small_art(&self) {
        let (url, bytes) = match &self.cover {
            Some((u, b)) => (u.clone(), b.clone()),
            None => return,
        };
        let (cols, rows) = self.art_dims();
        let png_px = if self.art_mode == ArtMode::Kitty { Some(256) } else { None };
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let r = art::render(bytes, cols, rows, png_px)
                .await
                .map_err(|e| format!("{e:#}"));
            let _ = tx.send(Msg::Art { url, art: r });
        });
    }

    /// Called after every draw: renders the big fullscreen cover when the layout asks for one.
    pub fn after_draw(&mut self) {
        if !self.fullscreen {
            if self.art_full.is_some() || self.full_req.is_some() {
                self.art_full = None;
                self.full_req = None;
            }
            return;
        }
        if self.art_mode == ArtMode::Off {
            return;
        }
        let want = match self.full_want {
            Some(w) => w,
            None => return,
        };
        let (url, bytes) = match &self.cover {
            Some((u, b)) => (u.clone(), b.clone()),
            None => return,
        };
        let key = (url.clone(), want);
        if self.full_req.as_ref() == Some(&key) {
            return;
        }
        self.full_req = Some(key);
        let png_px = if self.art_mode == ArtMode::Kitty { Some(480) } else { None };
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let r = art::render(bytes, want.0, want.1, png_px)
                .await
                .map_err(|e| format!("{e:#}"));
            let _ = tx.send(Msg::ArtFull { url, dims: want, art: r });
        });
    }

    fn on_track_changed(&mut self) {
        self.ensure_art();
        self.liked = None;
        let t = self.current_track().cloned();
        if let Some(t) = t {
            if !t.id.is_empty() && !t.is_episode {
                if let Some(api) = self.api.clone() {
                    let tx = self.tx.clone();
                    tokio::spawn(async move {
                        if let Ok(liked) = api.is_saved(&t.uri).await {
                            let _ = tx.send(Msg::Liked { id: t.id.clone(), liked });
                        }
                    });
                }
            }
        }
        if self.view == View::Lyrics || self.fullscreen {
            self.ensure_lyrics();
        }
        if self.view == View::Queue || self.fullscreen {
            self.load_queue();
        }
    }

    // ───────────────────────────── built-in player ─────────────────────────────

    /// Short status text for the playbar title.
    pub fn player_badge(&self) -> String {
        if !self.cfg.player.enabled {
            return String::new();
        }
        match &self.player_state {
            PlayerState::Off => String::new(),
            PlayerState::Starting => "built-in player: starting…".to_string(),
            PlayerState::NeedsLogin => "built-in player: sign-in needed (ctrl+p)".to_string(),
            PlayerState::Connecting => "built-in player: connecting…".to_string(),
            PlayerState::Ready => "built-in player: ready".to_string(),
            PlayerState::Failed(_) => "built-in player: failed (ctrl+p)".to_string(),
        }
    }

    fn start_player(&mut self) {
        if !self.cfg.player.enabled {
            self.player_state = PlayerState::Off;
            return;
        }
        self.stop_player();
        self.player_gen += 1;
        self.player_state = PlayerState::Starting;
        self.player_since = Instant::now();
        self.player_login_url = None;
        self.player_last_error.clear();
        self.player_log.clear();
        self.player_used_cache = player::has_cached_credentials();
        self.next_probe = Instant::now() + Duration::from_secs(3);
        match PlayerProc::spawn(&self.cfg.player, self.player_gen, self.tx.clone()) {
            Ok(p) => self.player = Some(p),
            Err(e) => {
                let msg = if e.kind() == std::io::ErrorKind::NotFound {
                    format!(
                        "`{}` was not found. Install it (Arch: sudo pacman -S librespot), set player.binary, or set player.enabled = false.",
                        self.cfg.player.binary
                    )
                } else {
                    format!("could not start `{}`: {e}", self.cfg.player.binary)
                };
                self.player_state = PlayerState::Failed(msg.clone());
                self.set_status(format!("Built-in player: {msg}"), true);
            }
        }
    }

    fn stop_player(&mut self) {
        if let Some(p) = self.player.take() {
            p.stop();
        }
        // Invalidate messages of the process we just stopped.
        self.player_gen += 1;
    }

    fn on_player_line(&mut self, line: String) {
        if self.player_log.len() >= 200 {
            self.player_log.pop_front();
        }
        self.player_log.push_back(line.clone());
        let lower = line.to_lowercase();
        if let Some(idx) = line.find("https://accounts.spotify.com") {
            let url = line[idx..].split_whitespace().next().unwrap_or("").to_string();
            if !url.is_empty() && self.player_state != PlayerState::Ready {
                self.player_login_url = Some(url);
                self.player_state = PlayerState::NeedsLogin;
                self.player_since = Instant::now();
                if self.popup == Popup::None {
                    self.popup = Popup::Player;
                }
            }
        } else if lower.contains("authenticated as")
            && matches!(self.player_state, PlayerState::Starting | PlayerState::NeedsLogin)
        {
            self.player_state = PlayerState::Connecting;
            self.player_since = Instant::now();
            self.player_login_url = None;
        }
        if line.contains("ERROR") || lower.starts_with("error") {
            self.player_last_error = line;
        }
    }

    fn on_player_exit(&mut self, code: Option<i32>) {
        self.player = None;
        let was_ready = self.player_state == PlayerState::Ready;
        let err = self.player_last_error.clone();
        let el = err.to_lowercase();
        let cred_problem = el.contains("invalid_credentials")
            || el.contains("bad credentials")
            || el.contains("credentials");
        if !was_ready && self.player_used_cache && !self.player_reset_done && cred_problem {
            // Saved login no longer accepted: forget it and sign in again once.
            self.player_reset_done = true;
            player::clear_credentials();
            self.set_status("Built-in player: saved login rejected, signing in again…", false);
            self.start_player();
            return;
        }
        if was_ready && self.player_restarts < 3 {
            self.player_restarts += 1;
            self.set_status("Built-in player stopped, restarting…", true);
            self.start_player();
            return;
        }
        let mut msg = match code {
            Some(c) => format!("librespot exited (code {c})"),
            None => "librespot exited".to_string(),
        };
        if !err.is_empty() {
            msg.push_str(": ");
            msg.push_str(&err);
        }
        if let Some(h) = player::hint_for(&err) {
            msg.push_str("\n\n");
            msg.push_str(h);
        }
        self.player_state = PlayerState::Failed(msg);
        self.set_status("Built-in player stopped. Press ctrl+p for details", true);
    }

    fn on_player_probe(&mut self, r: Result<Option<Device>, String>) {
        self.probe_in_flight = false;
        if let Ok(Some(dev)) = r {
            if matches!(
                self.player_state,
                PlayerState::Starting | PlayerState::NeedsLogin | PlayerState::Connecting
            ) {
                self.player_state = PlayerState::Ready;
                self.player_login_url = None;
                if self.popup == Popup::Player {
                    self.popup = Popup::None;
                }
                self.set_status("Built-in player ready: pick a track or press space to play here", false);
                if self.playback.is_none() {
                    let id = dev.id.clone();
                    self.cmd(move |api| async move { api.transfer(&id, false).await });
                }
            }
        }
    }

    /// Called every tick: looks for our device in Spotify's list while the player starts up.
    fn player_tick(&mut self) {
        let probing = matches!(
            self.player_state,
            PlayerState::Starting | PlayerState::NeedsLogin | PlayerState::Connecting
        );
        if !probing || self.screen != Screen::Main {
            return;
        }
        if !self.probe_in_flight && Instant::now() >= self.next_probe {
            if let Some(api) = self.api.clone() {
                self.probe_in_flight = true;
                self.next_probe = Instant::now() + Duration::from_secs(4);
                let name = self.cfg.player.device_name.to_lowercase();
                let tx = self.tx.clone();
                tokio::spawn(async move {
                    let r = api
                        .devices()
                        .await
                        .map(|d| d.into_iter().find(|x| x.name.to_lowercase() == name))
                        .map_err(|e| api::describe_error(&e));
                    let _ = tx.send(Msg::PlayerProbe(r));
                });
            }
        }
        let waiting_on_user = self.player_state == PlayerState::NeedsLogin;
        if !waiting_on_user && self.player_since.elapsed() > Duration::from_secs(90) {
            let err = self.player_last_error.clone();
            let mut msg = "The player started but never appeared in Spotify's device list (waited 90 s).".to_string();
            if !err.is_empty() {
                msg.push_str(&format!("\nLast error: {err}"));
            }
            if let Some(h) = player::hint_for(&err) {
                msg.push_str("\n\n");
                msg.push_str(h);
            }
            self.stop_player();
            self.player_state = PlayerState::Failed(msg);
            self.set_status("Built-in player did not start. Press ctrl+p for details", true);
        }
    }

    // ───────────────────────────── viz ─────────────────────────────

    fn start_viz(&mut self) {
        if !self.cfg.visualizer.enabled {
            return;
        }
        self.stop_viz();
        self.viz_error = None;
        let v = &self.cfg.visualizer;
        self.viz = Some(Viz::start(
            v.source.clone(),
            v.bars,
            v.fps,
            v.gain,
            v.decay,
            self.tx.clone(),
        ));
    }

    fn stop_viz(&mut self) {
        if let Some(v) = self.viz.take() {
            v.stop();
        }
    }

    /// The visualizer only runs while something shows it (its view, or fullscreen mode).
    fn sync_viz(&mut self) {
        let want = self.cfg.visualizer.enabled
            && self.screen == Screen::Main
            && (self.view == View::Visualizer || self.fullscreen);
        if want && self.viz.is_none() {
            self.start_viz();
        } else if !want && self.viz.is_some() {
            self.stop_viz();
        }
    }

    fn set_view(&mut self, v: View) {
        if self.view == v {
            return;
        }
        self.view = v;
        if v != View::Search {
            self.search.typing = false;
        }
        if v != View::Browse && v != View::Search {
            self.focus = Focus::Main;
        }
        match v {
            View::Lyrics => self.ensure_lyrics(),
            View::Queue => self.load_queue(),
            _ => {}
        }
        self.sync_viz();
    }

    fn toggle_fullscreen(&mut self) {
        if self.fullscreen {
            self.leave_fullscreen();
            return;
        }
        if self.current_track().is_none() {
            self.set_status("Nothing is playing yet. Start a track first", false);
            return;
        }
        self.fullscreen = true;
        self.search.typing = false;
        self.ensure_lyrics();
        self.load_queue();
        self.sync_viz();
    }

    fn leave_fullscreen(&mut self) {
        if self.fullscreen {
            self.fullscreen = false;
            self.sync_viz();
        }
    }

    // ───────────────────────────── message handling ─────────────────────────────

    pub fn update(&mut self, msg: Msg) {
        match msg {
            Msg::Input(ev) => match ev {
                Event::Key(k) => self.on_key(k),
                Event::Mouse(m) => self.on_mouse(m),
                Event::Paste(s) => self.on_paste(s),
                Event::Resize(_, _) => self.kitty_force_clear = true,
                _ => {}
            },
            Msg::Tick => self.on_tick(),
            Msg::LoggedIn(tok) => self.on_authenticated(tok),
            Msg::LoginFailed(e) => {
                self.login.phase = LoginPhase::Failed;
                self.login.error = Some(e);
            }
            Msg::AuthExpired => self.auth_expired(),
            Msg::Playback(pb) => {
                self.poll_in_flight = false;
                let old = self.current_track().map(|t| t.uri.clone());
                self.playback = pb;
                let new = self.current_track().map(|t| t.uri.clone());
                if old != new {
                    self.on_track_changed();
                }
            }
            Msg::PollFailed(e) => {
                self.poll_in_flight = false;
                self.set_status(e, true);
            }
            Msg::PollSoon => {
                self.next_poll = Instant::now() + Duration::from_millis(400);
            }
            Msg::Playlists(pls) => {
                let mut v = fixed_sidebar();
                v.extend(pls.into_iter().map(SidebarEntry::Playlist));
                self.sidebar = v;
                let bad = match self.sidebar_state.selected() {
                    Some(i) => i >= self.sidebar.len(),
                    None => true,
                };
                if bad {
                    self.sidebar_state.select(Some(0));
                }
            }
            Msg::Page { id, page } => {
                if let Some(c) = self.find_content_mut(id) {
                    c.loading = false;
                    c.error = None;
                    c.total = page.total;
                    c.next = page.next;
                    let mut items = page.items;
                    if matches!(c.source, Source::Album(_)) {
                        let name = c.title.clone();
                        for it in items.iter_mut() {
                            if let Item::Track(t) = it {
                                t.album = name.clone();
                            }
                        }
                    }
                    c.items.extend(items);
                    if c.state.selected().is_none() && !c.items.is_empty() {
                        c.state.select(Some(0));
                    }
                }
            }
            Msg::PageFailed { id, error, auth, status } => {
                if auth {
                    self.auth_expired();
                } else {
                    // Spotify refuses to list the tracks of playlists you do not own, but any
                    // playlist can still be played as a context. Do that instead of failing.
                    let fallback: Option<(String, String)> =
                        self.find_content_mut(id).and_then(|c| match &c.source {
                            Source::Playlist(pid) if status == 403 || status == 404 => {
                                Some((format!("spotify:playlist:{pid}"), c.title.clone()))
                            }
                            _ => None,
                        });
                    if let Some((uri, title)) = fallback {
                        let nested = (self.browse.len() > 1 && self.browse.iter().any(|c| c.id == id))
                            || (self.search_stack.len() > 1
                                && self.search_stack.iter().any(|c| c.id == id));
                        if nested {
                            self.browse.retain(|c| c.id != id);
                            self.search_stack.retain(|c| c.id != id);
                        } else if let Some(c) = self.find_content_mut(id) {
                            c.loading = false;
                            c.next = PageNext::None;
                            c.error = Some(
                                "Spotify does not let apps list the tracks of this playlist,\nso spotui plays it directly instead.\n\nPress P to play it again."
                                    .to_string(),
                            );
                        }
                        self.play_uri_context(uri, title);
                    } else if let Some(c) = self.find_content_mut(id) {
                        c.loading = false;
                        c.next = PageNext::None;
                        c.error = Some(error);
                    }
                }
            }
            Msg::Queue(r) => {
                self.queue.loading = false;
                match r {
                    Ok((cur, items)) => {
                        self.queue.current = cur;
                        self.queue.items = items;
                        self.queue.error = None;
                        let len = self.queue.items.len();
                        let bad = match self.queue.state.selected() {
                            Some(i) => i >= len,
                            None => true,
                        };
                        if bad {
                            self.queue.state.select(if len > 0 { Some(0) } else { None });
                        }
                    }
                    Err(e) => self.queue.error = Some(e),
                }
            }
            Msg::Devices(r) => {
                self.devices_loading = false;
                match r {
                    Ok(d) => {
                        let active = d.iter().position(|x| x.is_active).unwrap_or(0);
                        self.devices = d;
                        self.devices_state
                            .select(if self.devices.is_empty() { None } else { Some(active) });
                    }
                    Err(e) => {
                        self.popup = Popup::None;
                        self.set_status(e, true);
                    }
                }
            }
            Msg::Lyrics { uri, result } => {
                if self.lyrics.uri == uri {
                    match result {
                        Ok(Some(l)) => {
                            self.lyrics.lyrics = Some(l);
                            self.lyrics.status = LyricsStatus::Found;
                        }
                        Ok(None) => self.lyrics.status = LyricsStatus::NotFound,
                        Err(e) => self.lyrics.status = LyricsStatus::Failed(e),
                    }
                }
            }
            Msg::Cover { url, bytes } => {
                if self.art_url.as_deref() == Some(url.as_str()) {
                    match bytes {
                        Ok(b) => {
                            self.cover = Some((url, b));
                            self.render_small_art();
                        }
                        Err(_) => self.art = None,
                    }
                }
            }
            Msg::Art { url, art } => {
                if self.art_url.as_deref() == Some(url.as_str()) {
                    self.art = art.ok();
                }
            }
            Msg::ArtFull { url, dims, art } => {
                let current = self.art_url.as_deref() == Some(url.as_str())
                    && self.full_req.as_ref() == Some(&(url.clone(), dims));
                if current {
                    self.art_full = art.ok();
                }
            }
            Msg::Liked { id, liked } => {
                if self.current_track().map(|t| t.id == id).unwrap_or(false) {
                    self.liked = Some((id, liked));
                }
            }
            Msg::PlayerLine(gen, line) => {
                if gen == self.player_gen {
                    self.on_player_line(line);
                }
            }
            Msg::PlayerExit(gen, code) => {
                if gen == self.player_gen {
                    self.on_player_exit(code);
                }
            }
            Msg::PlayerProbe(r) => self.on_player_probe(r),
            Msg::Viz(b) => self.bars = b,
            Msg::VizError(e) => self.viz_error = Some(e),
            Msg::Error(e) => self.set_status(e, true),
        }
    }

    fn on_tick(&mut self) {
        self.tick = self.tick.wrapping_add(1);
        let expired = self
            .status
            .as_ref()
            .map(|s| Instant::now() >= s.until)
            .unwrap_or(false);
        if expired {
            self.status = None;
        }
        if self.screen == Screen::Main
            && self.api.is_some()
            && !self.poll_in_flight
            && Instant::now() >= self.next_poll
        {
            self.poll_now();
        }
        self.player_tick();
    }

    // ───────────────────────────── keyboard ─────────────────────────────

    fn on_key(&mut self, key: KeyEvent) {
        if key.kind == KeyEventKind::Release {
            return;
        }
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.should_quit = true;
            return;
        }
        match self.screen {
            Screen::Login => self.login_key(key),
            Screen::Main => self.main_key(key),
        }
    }

    fn on_paste(&mut self, s: String) {
        let clean: String = s.chars().filter(|c| !c.is_control()).collect();
        if self.screen == Screen::Login && self.login.phase == LoginPhase::NeedClientId {
            self.login.input.push_str(clean.trim());
        } else if self.screen == Screen::Main && self.view == View::Search && self.search.typing {
            self.search.input.push_str(&clean);
        }
    }

    fn login_key(&mut self, key: KeyEvent) {
        if self.login.phase == LoginPhase::NeedClientId {
            match key.code {
                KeyCode::Enter => self.submit_client_id(),
                KeyCode::Esc => self.should_quit = true,
                _ => {
                    let _ = edit_text(&mut self.login.input, &key);
                }
            }
            return;
        }
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => self.should_quit = true,
            KeyCode::Char('r') => self.start_login(),
            KeyCode::Char('o') => {
                let url = self.login.url.clone();
                open_url(&self.cfg.general.browser_command, &url);
            }
            KeyCode::Char('y') => {
                let url = self.login.url.clone();
                let ok = copy_to_clipboard(&url);
                self.login.error = Some(if ok {
                    "Link copied to the clipboard.".to_string()
                } else {
                    "No clipboard tool found (install wl-clipboard or xclip). Select the link with your mouse instead.".to_string()
                });
            }
            KeyCode::Char('c') => {
                if let Some(h) = self.login_task.take() {
                    h.abort();
                }
                self.login.input.clear();
                self.login.error = None;
                self.login.phase = LoginPhase::NeedClientId;
            }
            _ => {}
        }
    }

    fn main_key(&mut self, key: KeyEvent) {
        if self.popup != Popup::None {
            self.popup_key(key);
            return;
        }
        if self.view == View::Search && self.search.typing {
            self.search_typing_key(key);
            return;
        }
        if let Some(action) = self.keymap.lookup(&key) {
            self.dispatch(action);
        }
    }

    fn popup_key(&mut self, key: KeyEvent) {
        let act = self.keymap.lookup(&key);
        let close = key.code == KeyCode::Esc
            || matches!(act, Some(Action::Back) | Some(Action::Quit));
        match self.popup {
            Popup::Help => {
                if close || matches!(act, Some(Action::Help) | Some(Action::Select)) {
                    self.popup = Popup::None;
                }
            }
            Popup::Devices => {
                if close {
                    self.popup = Popup::None;
                } else if matches!(act, Some(Action::Up)) {
                    let len = self.devices.len();
                    move_list(&mut self.devices_state, len, -1);
                } else if matches!(act, Some(Action::Down)) {
                    let len = self.devices.len();
                    move_list(&mut self.devices_state, len, 1);
                } else if matches!(act, Some(Action::Select)) {
                    self.transfer_selected_device();
                }
            }
            Popup::Player => match key.code {
                KeyCode::Char('o') => {
                    if let Some(u) = self.player_login_url.clone() {
                        open_url(&self.cfg.general.browser_command, &u);
                    }
                }
                KeyCode::Char('y') => {
                    if let Some(u) = self.player_login_url.clone() {
                        let ok = copy_to_clipboard(&u);
                        self.set_status(
                            if ok { "Link copied to the clipboard" } else { "No clipboard tool found (install wl-clipboard or xclip)" },
                            !ok,
                        );
                    }
                }
                KeyCode::Char('r') => {
                    self.player_reset_done = false;
                    self.player_restarts = 0;
                    self.start_player();
                }
                KeyCode::Char('l') => {
                    player::clear_credentials();
                    self.player_reset_done = true;
                    self.player_restarts = 0;
                    self.start_player();
                }
                _ => {
                    if close {
                        self.popup = Popup::None;
                    }
                }
            },
            Popup::None => {}
        }
    }

    fn transfer_selected_device(&mut self) {
        let dev = self
            .devices_state
            .selected()
            .and_then(|i| self.devices.get(i))
            .cloned();
        if let Some(d) = dev {
            let play = self.playback.as_ref().map(|p| p.is_playing).unwrap_or(false);
            self.popup = Popup::None;
            self.set_status(format!("Switching to {}", d.name), false);
            let id = d.id.clone();
            self.cmd(move |api| async move { api.transfer(&id, play).await });
        }
    }

    fn search_typing_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => self.search.typing = false,
            KeyCode::Enter => {
                self.search.typing = false;
                let q = self.search.input.trim().to_string();
                if !q.is_empty() {
                    self.search.query = q;
                    self.run_search();
                    self.focus = Focus::Main;
                }
            }
            KeyCode::Tab => self.search.kind = self.search.kind.next(),
            _ => {
                let _ = edit_text(&mut self.search.input, &key);
            }
        }
    }

    fn run_search(&mut self) {
        let query = self.search.query.clone();
        if query.is_empty() {
            return;
        }
        let kind = self.search.kind;
        let id = self.new_id();
        let source = Source::Search { query: query.clone(), kind };
        self.search_stack.clear();
        self.search_stack
            .push(Content::new(id, format!("Search: {query}"), source.clone()));
        self.spawn_page(id, source, PageNext::None);
    }

    fn dispatch(&mut self, a: Action) {
        let step = self.cfg.general.volume_step as i32;
        let seek = self.cfg.general.seek_seconds as i64;
        match a {
            Action::Quit => self.should_quit = true,
            Action::Up => self.move_sel(-1),
            Action::Down => self.move_sel(1),
            Action::PageUp => self.move_sel(-10),
            Action::PageDown => self.move_sel(10),
            Action::Top => self.select_abs(true),
            Action::Bottom => self.select_abs(false),
            Action::Left => self.go_left(),
            Action::Right => self.go_right(),
            Action::Select => self.select_current(),
            Action::PlayContext => self.play_context(),
            Action::Back => self.back(),
            Action::PlayPause => self.play_pause(),
            Action::NextTrack => self.cmd(|api| async move { api.next_track().await }),
            Action::PrevTrack => self.cmd(|api| async move { api.prev_track().await }),
            Action::VolumeUp => self.change_volume(step),
            Action::VolumeDown => self.change_volume(-step),
            Action::SeekForward => self.seek_rel(seek),
            Action::SeekBack => self.seek_rel(-seek),
            Action::Shuffle => {
                let on = self.playback.as_ref().map(|p| p.shuffle).unwrap_or(false);
                if let Some(p) = self.playback.as_mut() {
                    p.shuffle = !on;
                }
                self.cmd(move |api| async move { api.shuffle(!on).await });
            }
            Action::Repeat => {
                let cur = self
                    .playback
                    .as_ref()
                    .map(|p| p.repeat.clone())
                    .unwrap_or_else(|| "off".to_string());
                let next = match cur.as_str() {
                    "off" => "context",
                    "context" => "track",
                    _ => "off",
                };
                if let Some(p) = self.playback.as_mut() {
                    p.repeat = next.to_string();
                }
                self.cmd(move |api| async move { api.repeat(next).await });
            }
            Action::Like => self.toggle_like(),
            Action::AddToQueue => self.add_to_queue(),
            Action::Fullscreen => self.toggle_fullscreen(),
            Action::Search => {
                self.leave_fullscreen();
                self.set_view(View::Search);
                self.search.typing = true;
                self.focus = Focus::Main;
            }
            Action::ViewBrowse => {
                self.leave_fullscreen();
                self.set_view(View::Browse);
            }
            Action::ViewQueue => self.toggle_view(View::Queue),
            Action::ViewLyrics => self.toggle_view(View::Lyrics),
            Action::ViewVisualizer => self.toggle_view(View::Visualizer),
            Action::Devices => self.open_devices(),
            Action::GotoAlbum => self.goto_album(),
            Action::GotoArtist => self.goto_artist(),
            Action::CycleFocus => {
                if self.view == View::Search {
                    self.search.kind = self.search.kind.next();
                    self.run_search();
                } else if self.sidebar_visible() {
                    self.focus = if self.focus == Focus::Sidebar { Focus::Main } else { Focus::Sidebar };
                }
            }
            Action::Refresh => self.refresh(),
            Action::Player => self.popup = Popup::Player,
            Action::Help => self.popup = Popup::Help,
        }
    }

    /// Opens a view, or returns to the library if it is already open.
    fn toggle_view(&mut self, v: View) {
        self.leave_fullscreen();
        if self.view == v {
            self.set_view(View::Browse);
        } else {
            self.set_view(v);
        }
    }

    fn move_sel(&mut self, delta: i32) {
        if self.fullscreen {
            let cur = self.lyrics.scroll as i64;
            self.lyrics.scroll = (cur + delta as i64).max(0) as usize;
            return;
        }
        if self.sidebar_focused() {
            let len = self.sidebar.len();
            move_list(&mut self.sidebar_state, len, delta);
            return;
        }
        match self.view {
            View::Browse | View::Search => {
                let stack = if self.view == View::Search {
                    &mut self.search_stack
                } else {
                    &mut self.browse
                };
                if let Some(c) = stack.last_mut() {
                    let len = c.items.len();
                    move_list(&mut c.state, len, delta);
                }
                self.after_move();
            }
            View::Queue => {
                let len = self.queue.items.len();
                move_list(&mut self.queue.state, len, delta);
            }
            View::Lyrics => {
                let cur = self.lyrics.scroll as i64;
                self.lyrics.scroll = (cur + delta as i64).max(0) as usize;
            }
            View::Visualizer => {}
        }
    }

    fn select_abs(&mut self, top: bool) {
        if self.fullscreen {
            if top {
                self.lyrics.scroll = 0;
            }
            return;
        }
        if self.sidebar_focused() {
            let len = self.sidebar.len();
            if len > 0 {
                self.sidebar_state.select(Some(if top { 0 } else { len - 1 }));
            }
            return;
        }
        match self.view {
            View::Browse | View::Search => {
                let stack = if self.view == View::Search {
                    &mut self.search_stack
                } else {
                    &mut self.browse
                };
                if let Some(c) = stack.last_mut() {
                    let len = c.items.len();
                    if len > 0 {
                        c.state.select(Some(if top { 0 } else { len - 1 }));
                    }
                }
                self.after_move();
            }
            View::Queue => {
                let len = self.queue.items.len();
                if len > 0 {
                    self.queue.state.select(Some(if top { 0 } else { len - 1 }));
                }
            }
            View::Lyrics => {
                if top {
                    self.lyrics.scroll = 0;
                }
            }
            View::Visualizer => {}
        }
    }

    fn go_left(&mut self) {
        if self.focus == Focus::Main && matches!(self.view, View::Browse | View::Search) {
            let len = self.cur_stack().len();
            if len > 1 {
                self.back();
                return;
            }
        }
        if self.sidebar_visible() {
            self.focus = Focus::Sidebar;
        }
    }

    fn go_right(&mut self) {
        if self.sidebar_focused() {
            self.select_current();
            return;
        }
        self.open_selected();
    }

    /// Opens the selected album / artist / playlist (tracks are ignored).
    fn open_selected(&mut self) {
        match self.selected_item() {
            Some((Item::Album(a), _)) => self.open_child(a.name.clone(), Source::Album(a.id.clone())),
            Some((Item::Artist(a), _)) => {
                self.open_child(a.name.clone(), Source::ArtistAlbums(a.id.clone()))
            }
            Some((Item::Playlist(p), _)) => {
                self.open_child(p.name.clone(), Source::Playlist(p.id.clone()))
            }
            _ => {}
        }
    }

    fn back(&mut self) {
        if self.fullscreen {
            self.leave_fullscreen();
            return;
        }
        match self.view {
            View::Browse | View::Search => {
                let is_search = self.view == View::Search;
                let stack = if is_search { &mut self.search_stack } else { &mut self.browse };
                if stack.len() > 1 {
                    stack.pop();
                } else if is_search {
                    self.set_view(View::Browse);
                }
            }
            _ => self.set_view(View::Browse),
        }
    }

    fn select_current(&mut self) {
        if self.sidebar_focused() {
            if let Some(i) = self.sidebar_state.selected() {
                self.open_sidebar_entry(i);
                self.focus = Focus::Main;
            }
            return;
        }
        if !matches!(self.view, View::Browse | View::Search) {
            return;
        }
        match self.selected_item() {
            Some((Item::Track(_), idx)) => self.play_track_at(idx),
            // Searched playlists: just play them (listing their tracks is not allowed by Spotify).
            Some((Item::Playlist(p), _)) if self.view == View::Search => {
                self.play_uri_context(p.uri.clone(), p.name.clone())
            }
            Some(_) => self.open_selected(),
            None => {}
        }
    }

    fn play_track_at(&mut self, idx: usize) {
        let built = {
            let stack = self.cur_stack();
            match stack.last() {
                Some(c) => match c.items.get(idx) {
                    Some(Item::Track(t)) => {
                        let body = if let Some(ctx) = &c.context_uri {
                            json!({ "context_uri": ctx, "offset": { "uri": t.uri } })
                        } else {
                            let uris: Vec<&str> = c.items[idx..]
                                .iter()
                                .filter_map(|it| match it {
                                    Item::Track(t) if !t.uri.is_empty() => Some(t.uri.as_str()),
                                    _ => None,
                                })
                                .take(50)
                                .collect();
                            json!({ "uris": uris })
                        };
                        Some((body, t.name.clone()))
                    }
                    _ => None,
                },
                None => None,
            }
        };
        if let Some((body, name)) = built {
            self.cmd(move |api| async move { api.play(Some(body)).await });
            self.set_status(format!("Playing {name}"), false);
        }
    }

    fn play_uri_context(&mut self, uri: String, label: String) {
        if uri.is_empty() {
            return;
        }
        self.cmd(move |api| async move { api.play(Some(json!({ "context_uri": uri }))).await });
        self.set_status(format!("Playing {label}"), false);
    }

    fn play_context(&mut self) {
        if self.sidebar_focused() {
            let entry = self
                .sidebar_state
                .selected()
                .and_then(|i| self.sidebar.get(i))
                .cloned();
            match entry {
                Some(SidebarEntry::Playlist(p)) => self.play_uri_context(p.uri.clone(), p.name.clone()),
                _ => self.set_status("Open it with Enter, then pick a track to play", false),
            }
            return;
        }
        match self.selected_item() {
            Some((Item::Track(_), idx)) => self.play_track_at(idx),
            Some((Item::Album(a), _)) => self.play_uri_context(a.uri.clone(), a.name.clone()),
            Some((Item::Playlist(p), _)) => self.play_uri_context(p.uri.clone(), p.name.clone()),
            Some((Item::Artist(a), _)) => self.play_uri_context(a.uri.clone(), a.name.clone()),
            None => {}
        }
    }

    fn play_pause(&mut self) {
        let playing = self.playback.as_ref().map(|p| p.is_playing).unwrap_or(false);
        let prog = self.progress_ms();
        if let Some(p) = self.playback.as_mut() {
            p.progress_ms = prog;
            p.fetched = Instant::now();
            p.is_playing = !playing;
        }
        if playing {
            self.cmd(|api| async move { api.pause().await });
        } else {
            self.cmd(|api| async move { api.resume().await });
        }
    }

    fn change_volume(&mut self, delta: i32) {
        let cur = self
            .playback
            .as_ref()
            .and_then(|p| p.device.as_ref())
            .and_then(|d| d.volume)
            .unwrap_or(50) as i32;
        let new = (cur + delta).clamp(0, 100) as u32;
        if let Some(p) = self.playback.as_mut() {
            if let Some(d) = p.device.as_mut() {
                d.volume = Some(new);
            }
        }
        self.cmd(move |api| async move { api.volume(new).await });
    }

    fn seek_rel(&mut self, secs: i64) {
        let dur = self.current_track().map(|t| t.duration_ms).unwrap_or(0);
        if dur == 0 {
            return;
        }
        let cur = self.progress_ms() as i64;
        let target = (cur + secs * 1000).clamp(0, dur as i64) as u64;
        self.seek_to(target);
    }

    fn seek_to(&mut self, ms: u64) {
        if let Some(p) = self.playback.as_mut() {
            p.progress_ms = ms;
            p.fetched = Instant::now();
        }
        self.cmd(move |api| async move { api.seek(ms).await });
    }

    fn toggle_like(&mut self) {
        let t = match self.current_track() {
            Some(t) if !t.id.is_empty() && !t.is_episode => t.clone(),
            _ => {
                self.set_status("Nothing to like right now", false);
                return;
            }
        };
        let currently = match &self.liked {
            Some((id, l)) if *id == t.id => *l,
            _ => false,
        };
        self.liked = Some((t.id.clone(), !currently));
        let uri = t.uri.clone();
        self.cmd(move |api| async move {
            if currently {
                api.unsave(&uri).await
            } else {
                api.save(&uri).await
            }
        });
        self.set_status(
            if currently { "Removed from Liked Songs" } else { "Saved to Liked Songs" },
            false,
        );
    }

    /// The track the user means: the selected one in a list, else the playing one.
    fn target_track(&self) -> Option<Track> {
        if self.focus == Focus::Main && matches!(self.view, View::Browse | View::Search) {
            if let Some((Item::Track(t), _)) = self.selected_item() {
                return Some(t);
            }
        }
        self.current_track().cloned()
    }

    fn add_to_queue(&mut self) {
        match self.selected_item() {
            Some((Item::Track(t), _)) if matches!(self.view, View::Browse | View::Search) => {
                let uri = t.uri.clone();
                self.cmd(move |api| async move { api.add_to_queue(&uri).await });
                self.set_status(format!("Added to queue: {}", t.name), false);
            }
            _ => self.set_status("Select a track in a list first", false),
        }
    }

    fn goto_album(&mut self) {
        let t = match self.target_track() {
            Some(t) => t,
            None => return,
        };
        if t.album_id.is_empty() {
            self.set_status("No album info for this item", false);
            return;
        }
        self.set_view(View::Browse);
        self.focus = Focus::Main;
        self.open_child(t.album.clone(), Source::Album(t.album_id.clone()));
    }

    fn goto_artist(&mut self) {
        let t = match self.target_track() {
            Some(t) => t,
            None => return,
        };
        let artist = t.artists.iter().find(|a| !a.0.is_empty()).cloned();
        match artist {
            Some((id, name)) => {
                self.set_view(View::Browse);
                self.focus = Focus::Main;
                self.open_child(name, Source::ArtistAlbums(id));
            }
            None => self.set_status("No artist info for this item", false),
        }
    }

    fn open_devices(&mut self) {
        let api = match self.api.clone() {
            Some(a) => a,
            None => return,
        };
        self.popup = Popup::Devices;
        self.devices_loading = true;
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let r = api.devices().await.map_err(|e| api::describe_error(&e));
            let _ = tx.send(Msg::Devices(r));
        });
    }

    fn refresh(&mut self) {
        self.load_playlists();
        let is_search = self.view == View::Search;
        let top = {
            let stack = if is_search { &mut self.search_stack } else { &mut self.browse };
            stack.pop().map(|c| (c.title, c.source))
        };
        if let Some((title, source)) = top {
            self.open_child(title, source);
        }
        if self.view == View::Queue {
            self.load_queue();
        }
        self.poll_now();
        self.set_status("Refreshing…", false);
    }

    // ───────────────────────────── mouse ─────────────────────────────

    fn is_double_click(&mut self, region: u8, idx: usize) -> bool {
        let now = Instant::now();
        let dbl = match self.last_click {
            Some((t, r, i)) => r == region && i == idx && now.duration_since(t) < Duration::from_millis(500),
            None => false,
        };
        self.last_click = if dbl { None } else { Some((now, region, idx)) };
        dbl
    }

    fn on_mouse(&mut self, m: MouseEvent) {
        if self.screen != Screen::Main || self.popup != Popup::None {
            return;
        }
        match m.kind {
            MouseEventKind::Down(MouseButton::Left) => self.click(m.column, m.row),
            MouseEventKind::ScrollUp => self.scroll_at(m.column, m.row, -3),
            MouseEventKind::ScrollDown => self.scroll_at(m.column, m.row, 3),
            _ => {}
        }
    }

    fn scroll_at(&mut self, x: u16, y: u16, delta: i32) {
        if hit(self.rects.sidebar, x, y) {
            self.focus = Focus::Sidebar;
        } else {
            self.focus = Focus::Main;
        }
        self.move_sel(delta);
    }

    fn click(&mut self, x: u16, y: u16) {
        let r = self.rects;
        if hit(r.progress, x, y) {
            let dur = self.current_track().map(|t| t.duration_ms).unwrap_or(0);
            if dur > 0 {
                let ratio = (x - r.progress.x) as f64 / r.progress.width.max(1) as f64;
                self.seek_to((dur as f64 * ratio) as u64);
            }
            return;
        }
        if hit(r.sidebar, x, y) {
            let idx = self.sidebar_state.offset() + (y - r.sidebar.y) as usize;
            if idx < self.sidebar.len() {
                self.focus = Focus::Sidebar;
                let dbl = self.is_double_click(1, idx);
                self.sidebar_state.select(Some(idx));
                if dbl {
                    self.select_current();
                }
            }
            return;
        }
        if hit(r.list, x, y) {
            let row = (y - r.list.y) as usize;
            self.focus = Focus::Main;
            match self.view {
                View::Browse | View::Search => {
                    let is_search = self.view == View::Search;
                    let stack = if is_search { &mut self.search_stack } else { &mut self.browse };
                    let mut picked: Option<usize> = None;
                    if let Some(c) = stack.last_mut() {
                        let idx = c.state.offset() + row;
                        if idx < c.items.len() {
                            c.state.select(Some(idx));
                            picked = Some(idx);
                        }
                    }
                    if let Some(idx) = picked {
                        if self.is_double_click(2, idx) {
                            self.select_current();
                        }
                    }
                }
                View::Queue => {
                    let idx = self.queue.state.offset() + row;
                    if idx < self.queue.items.len() {
                        self.queue.state.select(Some(idx));
                    }
                }
                _ => {}
            }
        }
    }
}
