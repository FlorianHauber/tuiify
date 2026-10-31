//! Spotify sign-in: Authorization Code flow with PKCE (no client secret needed).
//!
//! The TUI shows the authorization link (and opens the browser). A tiny HTTP server on
//! 127.0.0.1 catches the redirect, so the user only has to click "Agree" once.

use crate::api::ApiError;
use anyhow::{anyhow, Context, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rand::{distributions::Alphanumeric, Rng};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

pub const SCOPES: &str = "user-read-playback-state user-modify-playback-state \
user-read-currently-playing user-library-read user-library-modify \
playlist-read-private playlist-read-collaborative playlist-modify-public \
playlist-modify-private user-follow-read user-top-read user-read-recently-played";

const AUTH_URL: &str = "https://accounts.spotify.com/authorize";
const TOKEN_URL: &str = "https://accounts.spotify.com/api/token";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Token {
    pub access_token: String,
    pub refresh_token: String,
    /// Unix timestamp (seconds) at which the access token expires.
    pub expires_at: u64,
    pub client_id: String,
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl Token {
    pub fn is_expired(&self) -> bool {
        now_secs() + 60 >= self.expires_at
    }
}

// ───────────────────────────── token storage ─────────────────────────────

pub fn token_path() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("spotui")
        .join("token.json")
}

pub fn load_token() -> Option<Token> {
    let text = fs::read_to_string(token_path()).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn save_token(t: &Token) -> Result<()> {
    let path = token_path();
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let json = serde_json::to_string_pretty(t)?;
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&path)?;
        f.write_all(json.as_bytes())?;
    }
    #[cfg(not(unix))]
    {
        fs::write(&path, json)?;
    }
    Ok(())
}

pub fn delete_token() {
    let _ = fs::remove_file(token_path());
}

// ───────────────────────────── PKCE session ─────────────────────────────

pub struct PkceSession {
    pub verifier: String,
    pub state: String,
    pub url: String,
    pub redirect_uri: String,
}

pub fn new_session(client_id: &str, port: u16) -> PkceSession {
    let mut rng = rand::thread_rng();
    let verifier: String = (&mut rng)
        .sample_iter(&Alphanumeric)
        .take(64)
        .map(char::from)
        .collect();
    let state: String = (&mut rng)
        .sample_iter(&Alphanumeric)
        .take(16)
        .map(char::from)
        .collect();
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()).as_slice());
    let redirect_uri = format!("http://127.0.0.1:{port}/callback");
    let url = reqwest::Url::parse_with_params(
        AUTH_URL,
        &[
            ("client_id", client_id),
            ("response_type", "code"),
            ("redirect_uri", redirect_uri.as_str()),
            ("code_challenge_method", "S256"),
            ("code_challenge", challenge.as_str()),
            ("scope", SCOPES),
            ("state", state.as_str()),
        ],
    )
    .map(|u| u.to_string())
    .unwrap_or_default();
    PkceSession { verifier, state, url, redirect_uri }
}

/// Runs the whole login: wait for the browser redirect, then exchange the code for tokens.
pub async fn login(
    http: &reqwest::Client,
    client_id: &str,
    session: PkceSession,
    port: u16,
) -> Result<Token> {
    let code = wait_for_callback(port, &session.state).await?;
    exchange_code(http, client_id, &code, &session.verifier, &session.redirect_uri).await
}

async fn wait_for_callback(port: u16, expected_state: &str) -> Result<String> {
    let listener = TcpListener::bind(("127.0.0.1", port))
        .await
        .with_context(|| format!("cannot listen on 127.0.0.1:{port} (is the port already in use?)"))?;
    match tokio::time::timeout(Duration::from_secs(300), serve(&listener, expected_state)).await {
        Ok(r) => r,
        Err(_) => Err(anyhow!("login timed out after 5 minutes")),
    }
}

async fn serve(listener: &TcpListener, expected_state: &str) -> Result<String> {
    loop {
        let (mut sock, _) = listener.accept().await?;
        let mut buf = vec![0u8; 8192];
        let n = match tokio::time::timeout(Duration::from_secs(5), sock.read(&mut buf)).await {
            Ok(Ok(n)) => n,
            _ => continue,
        };
        let req = String::from_utf8_lossy(&buf[..n]).to_string();
        let first = req.lines().next().unwrap_or("");
        let mut parts = first.split_whitespace();
        let _method = parts.next();
        let target = parts.next().unwrap_or("/");
        if !target.starts_with("/callback") {
            respond(&mut sock, "404 Not Found", "Not found", "").await;
            continue;
        }
        let url = match reqwest::Url::parse(&format!("http://127.0.0.1{target}")) {
            Ok(u) => u,
            Err(_) => {
                respond(&mut sock, "400 Bad Request", "Bad request", "").await;
                continue;
            }
        };
        let mut code: Option<String> = None;
        let mut state: Option<String> = None;
        let mut error: Option<String> = None;
        for (k, v) in url.query_pairs() {
            match &*k {
                "code" => code = Some(v.to_string()),
                "state" => state = Some(v.to_string()),
                "error" => error = Some(v.to_string()),
                _ => {}
            }
        }
        if let Some(e) = error {
            respond(&mut sock, "200 OK", "Authorization failed", &format!("Spotify said: {e}. You can close this tab.")).await;
            return Err(anyhow!("authorization denied: {e}"));
        }
        if state.as_deref() != Some(expected_state) {
            respond(&mut sock, "400 Bad Request", "State mismatch", "Please restart the login in spotui.").await;
            return Err(anyhow!("state mismatch in the login callback (possible stale browser tab)"));
        }
        match code {
            Some(c) => {
                respond(&mut sock, "200 OK", "You're signed in", "You can close this tab and go back to spotui.").await;
                return Ok(c);
            }
            None => {
                respond(&mut sock, "400 Bad Request", "Missing code", "").await;
                continue;
            }
        }
    }
}

async fn respond(sock: &mut TcpStream, status: &str, title: &str, body: &str) {
    let html = format!(
        "<!doctype html><html><head><meta charset=utf-8><title>spotui</title>\
<style>body{{font-family:sans-serif;background:#121212;color:#fff;display:flex;\
align-items:center;justify-content:center;height:100vh;margin:0}}\
h2{{color:#1db954}}</style></head><body><div><h2>{title}</h2><p>{body}</p></div></body></html>"
    );
    let resp = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{html}",
        html.len()
    );
    let _ = sock.write_all(resp.as_bytes()).await;
    let _ = sock.shutdown().await;
}

// ───────────────────────────── token requests ─────────────────────────────

#[derive(Deserialize)]
struct TokenResp {
    access_token: String,
    expires_in: u64,
    refresh_token: Option<String>,
}

pub async fn exchange_code(
    http: &reqwest::Client,
    client_id: &str,
    code: &str,
    verifier: &str,
    redirect_uri: &str,
) -> Result<Token> {
    let resp = http
        .post(TOKEN_URL)
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", redirect_uri),
            ("client_id", client_id),
            ("code_verifier", verifier),
        ])
        .send()
        .await
        .context("could not reach accounts.spotify.com")?;
    parse_token_response(resp, client_id, None).await
}

pub async fn refresh(
    http: &reqwest::Client,
    client_id: &str,
    refresh_token: &str,
) -> Result<Token> {
    let resp = http
        .post(TOKEN_URL)
        .form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", client_id),
        ])
        .send()
        .await
        .context("could not reach accounts.spotify.com")?;
    parse_token_response(resp, client_id, Some(refresh_token)).await
}

async fn parse_token_response(
    resp: reqwest::Response,
    client_id: &str,
    old_refresh: Option<&str>,
) -> Result<Token> {
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        let v: serde_json::Value = serde_json::from_str(&text).unwrap_or(serde_json::Value::Null);
        let desc = v
            .get("error_description")
            .and_then(|x| x.as_str())
            .or_else(|| v.get("error").and_then(|x| x.as_str()))
            .unwrap_or("token request failed")
            .to_string();
        let code = if status.as_u16() == 400 || status.as_u16() == 401 { 401 } else { status.as_u16() };
        return Err(ApiError {
            status: code,
            reason: "AUTH".to_string(),
            message: format!("Spotify sign-in failed: {desc}"),
        }
        .into());
    }
    let tr: TokenResp = serde_json::from_str(&text).context("unexpected token response")?;
    let refresh_token = tr
        .refresh_token
        .or_else(|| old_refresh.map(|s| s.to_string()))
        .ok_or_else(|| anyhow!("Spotify did not return a refresh token"))?;
    Ok(Token {
        access_token: tr.access_token,
        refresh_token,
        expires_at: now_secs() + tr.expires_in,
        client_id: client_id.to_string(),
    })
}
