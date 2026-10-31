# spotui

A fast, keyboard-and-mouse driven Spotify TUI written in Rust. It browses your Spotify account
and controls playback on any Spotify device (desktop app, your phone, …). It can also run its own
**built-in player** (a managed `librespot`), so no other Spotify app has to be open.

Works on any Linux distribution; tested targets are Arch Linux and derivatives.

## Features

- Sign-in inside the TUI (OAuth PKCE, browser opens automatically, link can be copied)
- Library: Liked Songs, Recently Played, Top Tracks, Saved Albums, Followed Artists, your playlists
- Search (tracks / albums / artists / playlists), album and artist pages. Enter on a searched
  playlist plays it right away
- Fullscreen "now playing" (`f`): big cover, synced lyrics, up next, progress and a live visualizer
- Built-in player: spotui starts `librespot` itself and appears as the device "spotui" (see below)
- Playback: play/pause, next/previous, seek, volume, shuffle, repeat, queue, device switching, like/unlike
- Cover art: kitty graphics protocol (kitty, ghostty, WezTerm) or half-block fallback everywhere else
- Synced lyrics (LRCLIB)
- Audio visualizer (captures the system output via `parec`; PipeWire and PulseAudio)
- Vim-style keys, arrow keys and mouse (click, double-click, scroll, click the progress bar to seek)
- Everything configurable in one TOML file: keybindings, colors, layout, behavior

## Install on Arch Linux

```sh
sudo pacman -S --needed rust base-devel libpulse librespot   # libpulse: visualizer, librespot: built-in player
git clone <this repo> spotui && cd spotui
cargo build --release
sudo install -Dm755 target/release/spotui /usr/local/bin/spotui
```

On other distributions install a recent Rust toolchain (1.75+), a C compiler, and
`pulseaudio-utils` / `libpulse` for the visualizer.

To hear music you need either the built-in player (needs the `librespot` package) or Spotify
running on another device (`spotify-launcher`, your phone, …).

## Create your Spotify app (one time, ~2 minutes)

Spotify requires every third-party app to have its own Client ID:

1. Open <https://developer.spotify.com/dashboard> and create an app.
2. Add the Redirect URI `http://127.0.0.1:8888/callback` (the port is configurable).
3. Tick **Web API**, save, copy the **Client ID**.
4. Start `spotui`; it asks for the Client ID, saves it to the config and opens your browser.

**Spotify's current rules (since Feb 2026):** the app owner needs Premium, a Development Mode
app allows at most 5 authorized users (add them under *User Management*), and Premium is needed
to control playback. spotui cannot work around these limits.

## Built-in player (no Spotify app needed)

With `[player] enabled = true` (the default) spotui starts [librespot](https://github.com/librespot-org/librespot)
in the background. It shows up in Spotify as the device **spotui**, spotui controls it like any
other device, and when nothing is playing anywhere it selects it automatically.

- **First start:** a panel shows a sign-in link (and tries to open your browser). This is a
  separate one-time approval from spotui's own, because librespot signs in as Spotify's desktop
  app. The login is cached in `~/.local/share/spotui/librespot/` and reused afterwards.
- **Panel:** `ctrl+p` opens the player panel with its status, the link, the last errors and
  librespot's log. In it: `o` open link, `y` copy link, `r` restart, `l` forget the saved login.
- Audio stops when you quit spotui (librespot is stopped with it).
- Requires Spotify Premium. Options (device name, backend, bitrate, volume, …) are in `[player]`.
- The visualizer works best with the built-in player because the audio is local.

**Heads-up, as of October 2026:** Spotify changed its login system in August 2026 and librespot
playback broke for many users (open librespot issues #1737 and #1771, the latter a `503` from
Spotify's login5 service). Whether the built-in player works depends on that upstream situation,
not on spotui. If it fails, spotui shows librespot's exact error in the panel; updating the
`librespot` package or waiting for a fix is the remedy, and the rest of spotui (remote control
of your phone / desktop app) keeps working. You can switch the player off with
`[player] enabled = false`.

## Usage

```
spotui                       start
spotui --config my.toml      use another config
spotui --print-default-config
spotui --logout              forget the stored login
```

Press `?` in the app for all keys. Defaults:

| Key | Action | Key | Action |
|---|---|---|---|
| `j` `k` / arrows | move | `space` | play / pause |
| `h` `l` | back / open | `n` `p` | next / previous |
| `g` `G` | top / bottom | `+` `-` | volume |
| `enter` | play / open | `<` `>` | seek |
| `P` | play whole album/playlist/artist | `s` `r` | shuffle / repeat |
| `/` | search (`tab` changes type) | `L` | like / unlike |
| `a` | add to queue | `d` | choose device |
| `1` `q` `3` `4` | library / queue / lyrics / visualizer (press again to leave) | `A` `ctrl+a` | go to album / artist |
| `f` | **fullscreen** now playing (cover, lyrics, visualizer, up next) | `ctrl+c` | quit |
| `tab` | switch sidebar and list | `R` | refresh |
| `ctrl+p` | built-in player panel | `?` | all keys |

## Upgrading from an earlier version

Older versions wrote their default keys into your `config.toml`. On start spotui updates exactly
those old defaults (`q` quit → `ctrl+c`, `f` like → `L`, queue `2` → `q`/`2`, new `fullscreen = f`)
and keeps a backup as `config.toml.bak`. Keys you changed yourself are left alone; if one clashes
with a new default, spotui tells you in the status bar.

## Configuration

`~/.config/spotui/config.toml` is created on first start with every option documented.
Change any key binding by listing it under `[keybindings]`, for example:

```toml
[keybindings]
play_pause = ["space", "p"]
prev_track = ["P"]
quit = ["q", "ctrl+q"]
```

Colors accept names, `#rrggbb`, or 0–255. `background = "reset"` keeps your terminal's own
background/transparency.

## Limitations (deliberate, mostly Spotify's)

- **Playlist tracks** can only be *listed* for playlists you own or collaborate on (Spotify API
  rule since Feb 2026). Playing any playlist is still allowed, so Enter on a searched playlist
  plays it, and if opening one is refused spotui plays it instead. (Copying it into a new
  playlist is not possible: playlists cannot contain playlists and its tracks can't be read.)
- **Search** returns 10 results per page (Spotify limit); more load as you scroll.
- **No artist top-tracks / related artists / browse / new releases** – Spotify removed them.
- **Visualizer** only hears audio playing on the same computer (Spotify offers no audio feed).
  Set `visualizer.source` if the default monitor isn't right (`pactl list short sources`).
- **Sixel** cover art is not implemented yet; use `art.mode = "halfblock"` there.
  Inside tmux the kitty protocol is disabled automatically.
- Refresh tokens expire about six months after you approved the app; spotui then shows the
  sign-in screen again.

## Troubleshooting

| Problem | Fix |
|---|---|
| "No active Spotify device" | Wait for "built-in player: ready" in the playbar title, or start Spotify elsewhere and press `d` |
| Built-in player: `librespot` not found | `sudo pacman -S librespot` (or set `player.binary`) |
| Built-in player: failed / 503 / INVALID_CREDENTIALS | Press `ctrl+p`, then `l` to sign in again; see the upstream note above |
| No sound from the built-in player | Try `player.backend = "alsa"` or `""`; `librespot --backend ?` lists yours |
| "Premium required" / 403 | Playback control needs Premium; also check the dashboard's *User Management* |
| Login page says INVALID_CLIENT / redirect error | The Redirect URI in the dashboard must match exactly: `http://127.0.0.1:8888/callback` |
| Port 8888 in use | Change `redirect_port` in the config and in the dashboard |
| Visualizer: cannot start `parec` | `sudo pacman -S libpulse` |
| Colors look wrong | Use a truecolor terminal or set colors as 0–255 indexes |
