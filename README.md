# Spotify Tui for Arch Linux written in Rust

### For the spotify tui to work you need Spotify Premium!

#### !! This project was fully written by AI (Claude) !!

#### This project was only tested on Arch Linux it might work on other Distros or even Windows, but it isn't guaranteed!

## Installation 
```
sudo pacman -S --needed rust base-devel libpulse librespot
unzip tuiify.zip && cd spotui && cargo build --release
```

Go to the spotify developer page [here](https://developer.spotify.com/dashboard) and create an app. When creating the app, under redirect URL add http://127.0.0.1:8888/callback and under APIs used tick WebAPI.

After all of that you should be able to run ```./target/release/tuiify``` and a browser window should pop up where you can sign in after that you should have a functioning Spotify TUI
