//! Synced lyrics from LRCLIB (https://lrclib.net) — free, no API key.

use crate::models::Track;
use anyhow::Result;
use serde_json::Value;

#[derive(Clone, Debug)]
pub struct Lyrics {
    /// (timestamp in ms, text). For unsynced lyrics every timestamp is 0.
    pub lines: Vec<(u64, String)>,
    pub synced: bool,
}

fn parse_ts(tag: &str) -> Option<u64> {
    let (m, rest) = tag.split_once(':')?;
    let mins: u64 = m.trim().parse().ok()?;
    let secs: f64 = rest.trim().parse().ok()?;
    Some(mins * 60_000 + (secs * 1000.0).round() as u64)
}

/// Parses LRC text: `[mm:ss.xx] words`. Lines may carry several timestamps.
pub fn parse_lrc(s: &str) -> Vec<(u64, String)> {
    let mut out: Vec<(u64, String)> = Vec::new();
    for raw in s.lines() {
        let mut rest = raw.trim();
        let mut stamps: Vec<u64> = Vec::new();
        while rest.starts_with('[') {
            let end = match rest.find(']') {
                Some(e) => e,
                None => break,
            };
            if let Some(ts) = parse_ts(&rest[1..end]) {
                stamps.push(ts);
            }
            rest = rest[end + 1..].trim_start();
        }
        if stamps.is_empty() {
            continue;
        }
        for ts in stamps {
            out.push((ts, rest.to_string()));
        }
    }
    out.sort_by_key(|l| l.0);
    out
}

fn from_json(v: &Value) -> Option<Lyrics> {
    if let Some(s) = v.get("syncedLyrics").and_then(Value::as_str) {
        let lines = parse_lrc(s);
        if !lines.is_empty() {
            return Some(Lyrics { lines, synced: true });
        }
    }
    if let Some(s) = v.get("plainLyrics").and_then(Value::as_str) {
        let lines: Vec<(u64, String)> = s.lines().map(|l| (0u64, l.to_string())).collect();
        if !lines.is_empty() {
            return Some(Lyrics { lines, synced: false });
        }
    }
    None
}

pub async fn fetch(http: &reqwest::Client, track: &Track) -> Result<Option<Lyrics>> {
    if track.is_episode {
        return Ok(None);
    }
    let artist = track.primary_artist();
    let dur = (track.duration_ms / 1000).to_string();

    // 1) exact match
    let mut params: Vec<(&str, &str)> = vec![
        ("track_name", track.name.as_str()),
        ("artist_name", artist.as_str()),
        ("duration", dur.as_str()),
    ];
    if !track.album.is_empty() {
        params.push(("album_name", track.album.as_str()));
    }
    let resp = http
        .get("https://lrclib.net/api/get")
        .query(&params)
        .send()
        .await?;
    if resp.status().is_success() {
        let v: Value = resp.json().await?;
        if let Some(l) = from_json(&v) {
            return Ok(Some(l));
        }
    }

    // 2) fuzzy search, prefer a synced result
    let resp = http
        .get("https://lrclib.net/api/search")
        .query(&[("track_name", track.name.as_str()), ("artist_name", artist.as_str())])
        .send()
        .await?;
    if !resp.status().is_success() {
        return Ok(None);
    }
    let v: Value = resp.json().await?;
    let list: &[Value] = v.as_array().map(|a| a.as_slice()).unwrap_or(&[]);
    let want = track.duration_ms as i64 / 1000;
    let mut best: Option<Lyrics> = None;
    for entry in list {
        let d = entry.get("duration").and_then(Value::as_f64).unwrap_or(0.0) as i64;
        if want > 0 && d > 0 && (d - want).abs() > 5 {
            continue;
        }
        if let Some(l) = from_json(entry) {
            if l.synced {
                return Ok(Some(l));
            }
            if best.is_none() {
                best = Some(l);
            }
        }
    }
    Ok(best)
}
