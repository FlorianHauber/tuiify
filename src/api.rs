//! Spotify Web API client.
//!
//! Only uses endpoints that are still available to Development Mode apps after the
//! February 2026 changes (no batch endpoints, no browse/new-releases/top-tracks, search
//! limit <= 10, playlist `/items`, generic `/me/library`).

use crate::auth::{self, Token};
use crate::models::*;
use anyhow::{Context, Result};
use reqwest::{header, Client, Method, StatusCode};
use serde_json::{json, Value};
use std::fmt;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

const BASE: &str = "https://api.spotify.com/v1";

#[derive(Debug)]
pub struct ApiError {
    pub status: u16,
    pub reason: String,
    pub message: String,
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} (HTTP {})", self.message, self.status)
    }
}

impl std::error::Error for ApiError {}

impl ApiError {
    fn from_body(status: u16, body: &str) -> ApiError {
        let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
        let err = v.get("error");
        let message = match err {
            Some(e) if e.is_object() => e
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("request failed")
                .to_string(),
            Some(e) if e.is_string() => e.as_str().unwrap_or("request failed").to_string(),
            _ => {
                if body.trim().is_empty() {
                    "request failed".to_string()
                } else {
                    body.chars().take(160).collect()
                }
            }
        };
        let reason = err
            .and_then(|e| e.get("reason"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        ApiError { status, reason, message }
    }
}

pub fn api_error(e: &anyhow::Error) -> Option<&ApiError> {
    e.downcast_ref::<ApiError>()
}

pub fn is_auth_error(e: &anyhow::Error) -> bool {
    matches!(api_error(e), Some(a) if a.status == 401)
}

pub fn describe_error(e: &anyhow::Error) -> String {
    if let Some(a) = api_error(e) {
        return match (a.status, a.reason.as_str()) {
            (404, "NO_ACTIVE_DEVICE") => {
                "No active Spotify device. Open Spotify somewhere, then press `d` to pick a device".to_string()
            }
            (_, "NO_DEVICES") => a.message.clone(),
            (403, "PREMIUM_REQUIRED") => "Spotify Premium is required to control playback".to_string(),
            (429, _) => format!("Rate limited by Spotify: {}", a.message),
            _ => format!("{} (HTTP {})", a.message, a.status),
        };
    }
    format!("{e:#}")
}

fn no_active_device(e: &anyhow::Error) -> bool {
    matches!(api_error(e), Some(a) if a.status == 404 && a.reason == "NO_ACTIVE_DEVICE")
}

#[derive(Clone)]
pub struct Api {
    http: Client,
    token: Arc<Mutex<Token>>,
    preferred: String,
}

impl Api {
    pub fn new(http: Client, token: Token, preferred_device: String) -> Api {
        Api {
            http,
            token: Arc::new(Mutex::new(token)),
            preferred: preferred_device,
        }
    }

    async fn access_token(&self, force: bool) -> Result<String> {
        let mut t = self.token.lock().await;
        if force || t.is_expired() {
            let new = auth::refresh(&self.http, &t.client_id, &t.refresh_token).await?;
            let _ = auth::save_token(&new);
            *t = new;
        }
        Ok(t.access_token.clone())
    }

    /// Low-level request. Returns `Ok(None)` for empty (204/200-empty) responses.
    pub async fn request(
        &self,
        method: Method,
        path: &str,
        query: &[(&str, &str)],
        body: Option<Value>,
    ) -> Result<Option<Value>> {
        let url = format!("{BASE}{path}");
        let mut refreshed = false;
        let mut attempts = 0;
        loop {
            attempts += 1;
            let token = self.access_token(false).await?;
            let mut rb = self.http.request(method.clone(), &url).bearer_auth(token);
            if !query.is_empty() {
                rb = rb.query(query);
            }
            match &body {
                Some(b) => rb = rb.json(b),
                None => {
                    if method != Method::GET {
                        rb = rb.header(header::CONTENT_LENGTH, "0");
                    }
                }
            }
            let resp = rb.send().await.context("network error talking to Spotify")?;
            let status = resp.status();

            if status == StatusCode::UNAUTHORIZED && !refreshed {
                refreshed = true;
                self.access_token(true).await?;
                continue;
            }
            if status == StatusCode::TOO_MANY_REQUESTS {
                let wait = resp
                    .headers()
                    .get(header::RETRY_AFTER)
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.parse::<u64>().ok())
                    .unwrap_or(2);
                if wait <= 10 && attempts <= 3 {
                    tokio::time::sleep(Duration::from_secs(wait + 1)).await;
                    continue;
                }
                return Err(ApiError {
                    status: 429,
                    reason: "RATE_LIMIT".to_string(),
                    message: format!("try again in {wait}s"),
                }
                .into());
            }

            let text = resp.text().await.unwrap_or_default();
            if status.is_success() {
                if text.trim().is_empty() {
                    return Ok(None);
                }
                let v: Value = serde_json::from_str(&text).context("invalid JSON from Spotify")?;
                return Ok(Some(v));
            }
            return Err(ApiError::from_body(status.as_u16(), &text).into());
        }
    }

    async fn get(&self, path: &str, query: &[(&str, &str)]) -> Result<Value> {
        Ok(self
            .request(Method::GET, path, query, None)
            .await?
            .unwrap_or(Value::Null))
    }

    // ───────────────────────────── player ─────────────────────────────

    /// Runs a player command; if Spotify reports "no active device", wakes a device first.
    async fn player(
        &self,
        method: Method,
        path: &str,
        query: &[(&str, &str)],
        body: Option<Value>,
    ) -> Result<()> {
        match self.request(method.clone(), path, query, body.clone()).await {
            Ok(_) => Ok(()),
            Err(e) if no_active_device(&e) => {
                let dev = self.pick_device().await?;
                self.request(
                    Method::PUT,
                    "/me/player",
                    &[],
                    Some(json!({ "device_ids": [dev.id], "play": false })),
                )
                .await?;
                tokio::time::sleep(Duration::from_millis(800)).await;
                self.request(method, path, query, body).await?;
                Ok(())
            }
            Err(e) => Err(e),
        }
    }

    async fn pick_device(&self) -> Result<Device> {
        let devs = self.devices().await?;
        if devs.is_empty() {
            return Err(ApiError {
                status: 404,
                reason: "NO_DEVICES".to_string(),
                message: "No Spotify devices found. Start Spotify on this PC (spotify-launcher, spotifyd, ...) or on your phone, then try again.".to_string(),
            }
            .into());
        }
        let pref = self.preferred.to_lowercase();
        if !pref.is_empty() {
            if let Some(d) = devs.iter().find(|d| d.name.to_lowercase().contains(&pref)) {
                return Ok(d.clone());
            }
        }
        if let Some(d) = devs.iter().find(|d| d.is_active) {
            return Ok(d.clone());
        }
        Ok(devs
            .iter()
            .find(|d| !d.is_restricted)
            .unwrap_or(&devs[0])
            .clone())
    }

    pub async fn playback(&self) -> Result<Option<Playback>> {
        let v = self
            .request(Method::GET, "/me/player", &[("additional_types", "episode")], None)
            .await?;
        Ok(v.as_ref().and_then(parse_playback))
    }

    pub async fn devices(&self) -> Result<Vec<Device>> {
        let v = self.get("/me/player/devices", &[]).await?;
        Ok(arr_of(&v, "devices").iter().map(parse_device).collect())
    }

    pub async fn queue(&self) -> Result<(Option<Track>, Vec<Track>)> {
        let v = self.get("/me/player/queue", &[]).await?;
        let cur = v.get("currently_playing").and_then(parse_track);
        let items: Vec<Track> = arr_of(&v, "queue").iter().filter_map(parse_track).collect();
        Ok((cur, items))
    }

    pub async fn transfer(&self, device_id: &str, play: bool) -> Result<()> {
        self.request(
            Method::PUT,
            "/me/player",
            &[],
            Some(json!({ "device_ids": [device_id], "play": play })),
        )
        .await?;
        Ok(())
    }

    pub async fn play(&self, body: Option<Value>) -> Result<()> {
        self.player(Method::PUT, "/me/player/play", &[], body).await
    }

    pub async fn resume(&self) -> Result<()> {
        self.player(Method::PUT, "/me/player/play", &[], None).await
    }

    pub async fn pause(&self) -> Result<()> {
        self.player(Method::PUT, "/me/player/pause", &[], None).await
    }

    pub async fn next_track(&self) -> Result<()> {
        self.player(Method::POST, "/me/player/next", &[], None).await
    }

    pub async fn prev_track(&self) -> Result<()> {
        self.player(Method::POST, "/me/player/previous", &[], None).await
    }

    pub async fn seek(&self, ms: u64) -> Result<()> {
        let s = ms.to_string();
        self.player(Method::PUT, "/me/player/seek", &[("position_ms", s.as_str())], None)
            .await
    }

    pub async fn volume(&self, percent: u32) -> Result<()> {
        let s = percent.min(100).to_string();
        self.player(Method::PUT, "/me/player/volume", &[("volume_percent", s.as_str())], None)
            .await
    }

    pub async fn shuffle(&self, on: bool) -> Result<()> {
        let s = if on { "true" } else { "false" };
        self.player(Method::PUT, "/me/player/shuffle", &[("state", s)], None).await
    }

    pub async fn repeat(&self, state: &str) -> Result<()> {
        self.player(Method::PUT, "/me/player/repeat", &[("state", state)], None).await
    }

    pub async fn add_to_queue(&self, uri: &str) -> Result<()> {
        self.player(Method::POST, "/me/player/queue", &[("uri", uri)], None).await
    }

    // ───────────────────────────── library ─────────────────────────────

    pub async fn is_saved(&self, uri: &str) -> Result<bool> {
        let v = self.get("/me/library/contains", &[("uris", uri)]).await?;
        Ok(v.get(0).and_then(Value::as_bool).unwrap_or(false))
    }

    pub async fn save(&self, uri: &str) -> Result<()> {
        self.request(Method::PUT, "/me/library", &[("uris", uri)], None).await?;
        Ok(())
    }

    pub async fn unsave(&self, uri: &str) -> Result<()> {
        self.request(Method::DELETE, "/me/library", &[("uris", uri)], None).await?;
        Ok(())
    }

    pub async fn my_playlists(&self) -> Result<Vec<Playlist>> {
        let mut out = Vec::new();
        let mut offset = 0u32;
        for _ in 0..40 {
            let off = offset.to_string();
            let v = self
                .get("/me/playlists", &[("limit", "50"), ("offset", off.as_str())])
                .await?;
            let raw = arr_of(&v, "items");
            for it in raw {
                if let Some(p) = parse_playlist(it) {
                    out.push(p);
                }
            }
            let has_next = v.get("next").map(|n| n.is_string()).unwrap_or(false);
            if !has_next || raw.is_empty() {
                break;
            }
            offset += raw.len() as u32;
        }
        Ok(out)
    }

    // ───────────────────────────── paged lists ─────────────────────────────

    pub async fn fetch_page(&self, source: &Source, next: &PageNext) -> Result<Page> {
        let offset = match next {
            PageNext::Offset(o) => *o,
            _ => 0,
        };
        let off = offset.to_string();
        match source {
            Source::Liked => {
                let v = self
                    .get("/me/tracks", &[("limit", "50"), ("offset", off.as_str())])
                    .await?;
                Ok(parse_paging(&v, offset, |x: &Value| {
                    parse_track(x.get("track")?).map(Item::Track)
                }))
            }
            Source::Recent => {
                let v = self.get("/me/player/recently-played", &[("limit", "50")]).await?;
                let mut page = parse_paging(&v, 0, |x: &Value| {
                    parse_track(x.get("track")?).map(Item::Track)
                });
                page.next = PageNext::None;
                Ok(page)
            }
            Source::TopTracks => {
                let v = self
                    .get(
                        "/me/top/tracks",
                        &[("limit", "50"), ("time_range", "medium_term"), ("offset", off.as_str())],
                    )
                    .await?;
                Ok(parse_paging(&v, offset, |x: &Value| parse_track(x).map(Item::Track)))
            }
            Source::SavedAlbums => {
                let v = self
                    .get("/me/albums", &[("limit", "50"), ("offset", off.as_str())])
                    .await?;
                Ok(parse_paging(&v, offset, |x: &Value| {
                    parse_album(x.get("album")?).map(Item::Album)
                }))
            }
            Source::FollowedArtists => {
                let after = match next {
                    PageNext::Cursor(c) => c.clone(),
                    _ => String::new(),
                };
                let mut q: Vec<(&str, &str)> = vec![("type", "artist"), ("limit", "50")];
                if !after.is_empty() {
                    q.push(("after", after.as_str()));
                }
                let v = self.get("/me/following", &q).await?;
                let section = v.get("artists").cloned().unwrap_or(Value::Null);
                let mut page = parse_paging(&section, 0, |x: &Value| parse_artist(x).map(Item::Artist));
                let cursor = section
                    .get("cursors")
                    .and_then(|c| c.get("after"))
                    .and_then(Value::as_str)
                    .map(|s| s.to_string());
                page.next = match cursor {
                    Some(c) if !c.is_empty() && !page.items.is_empty() => PageNext::Cursor(c),
                    _ => PageNext::None,
                };
                Ok(page)
            }
            Source::Playlist(id) => {
                let path = format!("/playlists/{id}/items");
                let v = self
                    .get(&path, &[("limit", "50"), ("offset", off.as_str())])
                    .await?;
                // February 2026 renamed `track` -> `item`; accept both.
                Ok(parse_paging(&v, offset, |x: &Value| {
                    let t = x.get("item").or_else(|| x.get("track"))?;
                    parse_track(t).map(Item::Track)
                }))
            }
            Source::Album(id) => {
                let path = format!("/albums/{id}/tracks");
                let v = self
                    .get(&path, &[("limit", "50"), ("offset", off.as_str())])
                    .await?;
                Ok(parse_paging(&v, offset, |x: &Value| parse_track(x).map(Item::Track)))
            }
            Source::ArtistAlbums(id) => {
                let path = format!("/artists/{id}/albums");
                let v = self
                    .get(
                        &path,
                        &[
                            ("include_groups", "album,single"),
                            ("limit", "50"),
                            ("offset", off.as_str()),
                        ],
                    )
                    .await?;
                Ok(parse_paging(&v, offset, |x: &Value| parse_album(x).map(Item::Album)))
            }
            Source::Search { query, kind } => {
                let (typ, key) = match kind {
                    SearchKind::Track => ("track", "tracks"),
                    SearchKind::Album => ("album", "albums"),
                    SearchKind::Artist => ("artist", "artists"),
                    SearchKind::Playlist => ("playlist", "playlists"),
                };
                // Spotify caps search at 10 results per request since February 2026.
                let v = self
                    .get(
                        "/search",
                        &[("q", query.as_str()), ("type", typ), ("limit", "10"), ("offset", off.as_str())],
                    )
                    .await?;
                let section = v.get(key).cloned().unwrap_or(Value::Null);
                Ok(match kind {
                    SearchKind::Track => {
                        parse_paging(&section, offset, |x: &Value| parse_track(x).map(Item::Track))
                    }
                    SearchKind::Album => {
                        parse_paging(&section, offset, |x: &Value| parse_album(x).map(Item::Album))
                    }
                    SearchKind::Artist => {
                        parse_paging(&section, offset, |x: &Value| parse_artist(x).map(Item::Artist))
                    }
                    SearchKind::Playlist => parse_paging(&section, offset, |x: &Value| {
                        parse_playlist(x).map(Item::Playlist)
                    }),
                })
            }
        }
    }
}
