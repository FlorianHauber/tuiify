//! Domain types and lenient parsing of Spotify JSON.
//!
//! Spotify's API changes often (see the February 2026 changes), so instead of strict
//! serde structs we pull what we need out of `serde_json::Value` and tolerate missing
//! fields, nulls and renamed keys.

use serde_json::Value;
use std::time::Instant;

// ───────────────────────────── JSON helpers ─────────────────────────────

pub fn str_of(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or("").to_string()
}

pub fn u64_of(v: &Value, key: &str) -> u64 {
    v.get(key).and_then(Value::as_u64).unwrap_or(0)
}

pub fn bool_of(v: &Value, key: &str) -> bool {
    v.get(key).and_then(Value::as_bool).unwrap_or(false)
}

pub fn arr_of<'a>(v: &'a Value, key: &str) -> &'a [Value] {
    match v.get(key).and_then(Value::as_array) {
        Some(a) => a.as_slice(),
        None => &[],
    }
}

/// Picks the smallest image that is at least `min_w` wide, otherwise the largest one.
fn pick_image(imgs: &[Value], min_w: u64) -> Option<String> {
    let mut best: Option<(u64, &str)> = None;
    for im in imgs {
        let url = match im.get("url").and_then(Value::as_str) {
            Some(u) => u,
            None => continue,
        };
        let w = im.get("width").and_then(Value::as_u64).unwrap_or(0);
        best = match best {
            None => Some((w, url)),
            Some((bw, bu)) => {
                let better = if bw >= min_w {
                    w >= min_w && w < bw
                } else {
                    w > bw
                };
                if better {
                    Some((w, url))
                } else {
                    Some((bw, bu))
                }
            }
        };
    }
    best.map(|b| b.1.to_string())
}

// ───────────────────────────── domain types ─────────────────────────────

#[derive(Clone, Debug, Default)]
pub struct Track {
    pub id: String,
    pub uri: String,
    pub name: String,
    /// (artist id, artist name)
    pub artists: Vec<(String, String)>,
    pub album: String,
    pub album_id: String,
    pub duration_ms: u64,
    pub image_url: Option<String>,
    pub is_episode: bool,
    pub explicit: bool,
}

impl Track {
    pub fn artist_line(&self) -> String {
        self.artists
            .iter()
            .map(|a| a.1.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    }

    pub fn primary_artist(&self) -> String {
        self.artists.first().map(|a| a.1.clone()).unwrap_or_default()
    }
}

#[derive(Clone, Debug, Default)]
pub struct Album {
    pub id: String,
    pub uri: String,
    pub name: String,
    pub artist: String,
    pub year: String,
    pub total_tracks: u64,
}

#[derive(Clone, Debug, Default)]
pub struct Artist {
    pub id: String,
    pub uri: String,
    pub name: String,
}

#[derive(Clone, Debug, Default)]
pub struct Playlist {
    pub id: String,
    pub uri: String,
    pub name: String,
    pub owner: String,
    pub total: u64,
}

#[derive(Clone, Debug, Default)]
pub struct Device {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub is_active: bool,
    pub is_restricted: bool,
    pub volume: Option<u32>,
}

#[derive(Clone, Debug)]
pub struct Playback {
    pub is_playing: bool,
    pub progress_ms: u64,
    pub track: Option<Track>,
    pub device: Option<Device>,
    pub shuffle: bool,
    /// "off" | "context" | "track"
    pub repeat: String,
    pub fetched: Instant,
}

#[derive(Clone, Debug)]
pub enum Item {
    Track(Track),
    Album(Album),
    Artist(Artist),
    Playlist(Playlist),
}

#[derive(Clone, Debug, PartialEq)]
pub enum PageNext {
    None,
    Offset(u32),
    Cursor(String),
}

#[derive(Clone, Debug)]
pub struct Page {
    pub items: Vec<Item>,
    pub next: PageNext,
    pub total: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchKind {
    Track,
    Album,
    Artist,
    Playlist,
}

impl SearchKind {
    pub const ALL: [SearchKind; 4] = [
        SearchKind::Track,
        SearchKind::Album,
        SearchKind::Artist,
        SearchKind::Playlist,
    ];

    pub fn label(self) -> &'static str {
        match self {
            SearchKind::Track => "Tracks",
            SearchKind::Album => "Albums",
            SearchKind::Artist => "Artists",
            SearchKind::Playlist => "Playlists",
        }
    }

    pub fn next(self) -> SearchKind {
        match self {
            SearchKind::Track => SearchKind::Album,
            SearchKind::Album => SearchKind::Artist,
            SearchKind::Artist => SearchKind::Playlist,
            SearchKind::Playlist => SearchKind::Track,
        }
    }
}

/// Where a list of items comes from (used for lazy paging).
#[derive(Clone, Debug)]
pub enum Source {
    Liked,
    Recent,
    TopTracks,
    SavedAlbums,
    FollowedArtists,
    Playlist(String),
    Album(String),
    ArtistAlbums(String),
    Search { query: String, kind: SearchKind },
}

// ───────────────────────────── parsing ─────────────────────────────

pub fn parse_track(v: &Value) -> Option<Track> {
    if !v.is_object() {
        return None;
    }
    let uri = str_of(v, "uri");
    let name = str_of(v, "name");
    if uri.is_empty() && name.is_empty() {
        return None;
    }
    let is_episode = str_of(v, "type") == "episode";
    let mut t = Track {
        id: str_of(v, "id"),
        uri,
        name,
        duration_ms: u64_of(v, "duration_ms"),
        is_episode,
        explicit: bool_of(v, "explicit"),
        ..Default::default()
    };
    if is_episode {
        let show = v.get("show");
        let show_name = show.map(|s| str_of(s, "name")).unwrap_or_default();
        t.artists = vec![(String::new(), show_name.clone())];
        t.album = show_name;
        t.image_url = pick_image(arr_of(v, "images"), 300)
            .or_else(|| show.and_then(|s| pick_image(arr_of(s, "images"), 300)));
    } else {
        for a in arr_of(v, "artists") {
            t.artists.push((str_of(a, "id"), str_of(a, "name")));
        }
        if let Some(al) = v.get("album") {
            t.album = str_of(al, "name");
            t.album_id = str_of(al, "id");
            t.image_url = pick_image(arr_of(al, "images"), 300);
        }
    }
    Some(t)
}

pub fn parse_album(v: &Value) -> Option<Album> {
    if !v.is_object() {
        return None;
    }
    let id = str_of(v, "id");
    if id.is_empty() {
        return None;
    }
    let artist = arr_of(v, "artists")
        .iter()
        .map(|a| str_of(a, "name"))
        .collect::<Vec<_>>()
        .join(", ");
    let year: String = str_of(v, "release_date").chars().take(4).collect();
    Some(Album {
        id,
        uri: str_of(v, "uri"),
        name: str_of(v, "name"),
        artist,
        year,
        total_tracks: u64_of(v, "total_tracks"),
    })
}

pub fn parse_artist(v: &Value) -> Option<Artist> {
    if !v.is_object() {
        return None;
    }
    let id = str_of(v, "id");
    if id.is_empty() {
        return None;
    }
    Some(Artist {
        id,
        uri: str_of(v, "uri"),
        name: str_of(v, "name"),
    })
}

pub fn parse_playlist(v: &Value) -> Option<Playlist> {
    if !v.is_object() {
        return None;
    }
    let id = str_of(v, "id");
    if id.is_empty() {
        return None;
    }
    // February 2026: `tracks` was renamed to `items`. Accept both.
    let total = v
        .get("items")
        .and_then(|i| i.get("total"))
        .and_then(Value::as_u64)
        .or_else(|| {
            v.get("tracks")
                .and_then(|i| i.get("total"))
                .and_then(Value::as_u64)
        })
        .unwrap_or(0);
    let owner = v
        .get("owner")
        .map(|o| {
            let n = str_of(o, "display_name");
            if n.is_empty() {
                str_of(o, "id")
            } else {
                n
            }
        })
        .unwrap_or_default();
    Some(Playlist {
        id,
        uri: str_of(v, "uri"),
        name: str_of(v, "name"),
        owner,
        total,
    })
}

pub fn parse_device(v: &Value) -> Device {
    Device {
        id: str_of(v, "id"),
        name: str_of(v, "name"),
        kind: str_of(v, "type"),
        is_active: bool_of(v, "is_active"),
        is_restricted: bool_of(v, "is_restricted"),
        volume: v
            .get("volume_percent")
            .and_then(Value::as_u64)
            .map(|x| x as u32),
    }
}

pub fn parse_playback(v: &Value) -> Option<Playback> {
    if !v.is_object() {
        return None;
    }
    let track = v.get("item").and_then(parse_track);
    let device = v
        .get("device")
        .filter(|d| d.is_object())
        .map(parse_device);
    let repeat = {
        let r = str_of(v, "repeat_state");
        if r.is_empty() {
            "off".to_string()
        } else {
            r
        }
    };
    Some(Playback {
        is_playing: bool_of(v, "is_playing"),
        progress_ms: u64_of(v, "progress_ms"),
        track,
        device,
        shuffle: bool_of(v, "shuffle_state"),
        repeat,
        fetched: Instant::now(),
    })
}

/// Parses a Spotify paging object using `f` to convert each element.
pub fn parse_paging<F>(v: &Value, offset: u32, f: F) -> Page
where
    F: Fn(&Value) -> Option<Item>,
{
    let raw = arr_of(v, "items");
    let items: Vec<Item> = raw.iter().filter_map(|x| f(x)).collect();
    let total = u64_of(v, "total") as u32;
    let has_next = v.get("next").map(|n| n.is_string()).unwrap_or(false);
    let next = if has_next && !raw.is_empty() {
        PageNext::Offset(offset + raw.len() as u32)
    } else {
        PageNext::None
    };
    Page { items, next, total }
}
