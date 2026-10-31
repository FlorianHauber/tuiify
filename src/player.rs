//! Built-in player: runs `librespot` as a managed child process so spotui shows up as a
//! Spotify Connect device ("spotui") and can play audio without any other Spotify app.
//!
//! spotui keeps controlling playback through the Web API exactly like before; librespot just
//! provides the device that actually makes sound.
//!
//! Login uses librespot's own OAuth flow (`--enable-oauth`), cached in spotui's data dir, so the
//! browser approval is only needed once. Premium is required (a librespot limitation).

use crate::app::Msg;
use crate::config::PlayerCfg;
use std::fs;
use std::io::{self, BufRead, BufReader, Read};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc::UnboundedSender;

/// Where librespot keeps its login credentials and volume (`--system-cache`).
pub fn data_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("spotui")
        .join("librespot")
}

pub fn has_cached_credentials() -> bool {
    data_dir().join("credentials.json").exists()
}

pub fn clear_credentials() {
    let _ = fs::remove_file(data_dir().join("credentials.json"));
}

pub struct PlayerProc {
    child: Arc<Mutex<Child>>,
}

impl PlayerProc {
    pub fn spawn(cfg: &PlayerCfg, gen: u64, tx: UnboundedSender<Msg>) -> io::Result<PlayerProc> {
        let dir = data_dir();
        fs::create_dir_all(&dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&dir, fs::Permissions::from_mode(0o700));
        }

        let mut args: Vec<String> = Vec::new();
        let mut push = |k: &str, v: String| {
            args.push(k.to_string());
            args.push(v);
        };
        push("--name", cfg.device_name.clone());
        push("--device-type", cfg.device_type.clone());
        push("--system-cache", dir.to_string_lossy().to_string());
        push("--bitrate", cfg.bitrate.to_string());
        push("--initial-volume", cfg.initial_volume.min(100).to_string());
        if !cfg.backend.trim().is_empty() {
            push("--backend", cfg.backend.trim().to_string());
        }
        if !cfg.device.trim().is_empty() {
            push("--device", cfg.device.trim().to_string());
        }
        if cfg.normalisation {
            args.push("--enable-volume-normalisation".to_string());
        }
        if cfg.auth.trim().eq_ignore_ascii_case("zeroconf") {
            // Wait for the official Spotify app (same network) to hand over a login.
        } else {
            args.push("--enable-oauth".to_string());
            args.push("--disable-discovery".to_string());
        }
        args.extend(cfg.extra_args.iter().cloned());

        let mut cmd = Command::new(cfg.binary.trim());
        cmd.args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        // If spotui dies (even by SIGKILL), make sure librespot does not linger.
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::process::CommandExt;
            unsafe {
                cmd.pre_exec(|| {
                    libc::prctl(
                        libc::PR_SET_PDEATHSIG,
                        libc::SIGTERM as libc::c_ulong,
                        0 as libc::c_ulong,
                        0 as libc::c_ulong,
                        0 as libc::c_ulong,
                    );
                    Ok(())
                });
            }
        }

        let mut child = cmd.spawn()?;
        if let Some(out) = child.stdout.take() {
            pump(out, gen, tx.clone());
        }
        if let Some(err) = child.stderr.take() {
            pump(err, gen, tx.clone());
        }
        let child = Arc::new(Mutex::new(child));

        // Watch for exit.
        let watched = child.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_millis(400));
            let status = match watched.lock() {
                Ok(mut c) => c.try_wait(),
                Err(_) => break,
            };
            match status {
                Ok(Some(st)) => {
                    let _ = tx.send(Msg::PlayerExit(gen, st.code()));
                    break;
                }
                Ok(None) => continue,
                Err(_) => {
                    let _ = tx.send(Msg::PlayerExit(gen, None));
                    break;
                }
            }
        });

        Ok(PlayerProc { child })
    }

    pub fn stop(&self) {
        if let Ok(mut c) = self.child.lock() {
            let _ = c.kill();
        }
    }
}

/// Forwards every output line of the child to the app.
fn pump<R: Read + Send + 'static>(r: R, gen: u64, tx: UnboundedSender<Msg>) {
    std::thread::spawn(move || {
        let mut rd = BufReader::new(r);
        let mut buf: Vec<u8> = Vec::new();
        loop {
            buf.clear();
            match rd.read_until(b'\n', &mut buf) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    let line = String::from_utf8_lossy(&buf).trim_end().to_string();
                    if !line.is_empty() && tx.send(Msg::PlayerLine(gen, line)).is_err() {
                        break;
                    }
                }
            }
        }
    });
}

/// Human-friendly explanation for common librespot failures.
pub fn hint_for(err: &str) -> Option<&'static str> {
    let l = err.to_lowercase();
    if l.contains("invalid_credentials") || l.contains("bad credentials") {
        Some("Spotify rejected librespot's login. Press `l` in this panel to sign in again. Since August 2026 Spotify has been changing the login flow librespot relies on, so this can also be an upstream problem (see librespot issues #1737 and #1771).")
    } else if l.contains("503") || l.contains("service unavailable") {
        Some("Spotify answered 503 (service unavailable). That is either a temporary Spotify outage or the upstream librespot problem tracked in librespot issue #1771. Try again later, or update librespot.")
    } else if l.contains("backend") {
        Some("librespot could not use the audio backend. Set player.backend in the config (try \"pulseaudio\", \"alsa\", or \"\" for the default; `librespot --backend ?` lists the ones your build has).")
    } else if l.contains("address already in use") {
        Some("A port is already in use (librespot's login uses port 5588). Close the other librespot and press `r`.")
    } else {
        None
    }
}
