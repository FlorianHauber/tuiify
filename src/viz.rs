//! Audio visualizer. Captures the system output monitor with `parec` (libpulse; works on
//! PulseAudio and PipeWire) and turns it into spectrum bars with an FFT.
//!
//! Spotify does not expose raw audio or (for new apps) audio analysis, so this can only
//! visualize audio that is playing on *this* machine.

use crate::app::Msg;
use rustfft::{num_complex::Complex, FftPlanner};
use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc::UnboundedSender;

const N: usize = 2048;
const HOP: usize = 1024;
const RATE: f32 = 44100.0;

pub struct Viz {
    stop: Arc<AtomicBool>,
}

impl Viz {
    pub fn start(
        source: String,
        bars: usize,
        fps: u64,
        gain: f32,
        decay: f32,
        tx: UnboundedSender<Msg>,
    ) -> Viz {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let bars = bars.clamp(8, 256);
        let frame = Duration::from_millis(1000 / fps.clamp(5, 120));
        let decay = decay.clamp(0.0, 0.99);
        std::thread::spawn(move || run(source, bars, frame, gain, decay, tx, flag));
        Viz { stop }
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn run(
    source: String,
    bars: usize,
    frame: Duration,
    gain: f32,
    decay: f32,
    tx: UnboundedSender<Msg>,
    stop: Arc<AtomicBool>,
) {
    let args = vec![
        format!("--device={source}"),
        "--format=s16le".to_string(),
        "--rate=44100".to_string(),
        "--channels=1".to_string(),
        "--latency-msec=20".to_string(),
        "--raw".to_string(),
    ];
    let mut child = match Command::new("parec")
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            let _ = tx.send(Msg::VizError(format!(
                "cannot start `parec` ({e}). Install it: sudo pacman -S libpulse"
            )));
            return;
        }
    };
    let mut stdout = match child.stdout.take() {
        Some(s) => s,
        None => {
            let _ = child.kill();
            return;
        }
    };

    let mut planner = FftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(N);
    let window: Vec<f32> = (0..N)
        .map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / (N as f32 - 1.0)).cos())
        .collect();
    let mut samples = vec![0f32; N];
    let mut raw = vec![0u8; HOP * 2];
    let mut smooth = vec![0f32; bars];
    let mut last_sent = Instant::now();
    let (f_lo, f_hi) = (40.0f32, 16000.0f32);
    let bin_hz = RATE / N as f32;

    loop {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        if stdout.read_exact(&mut raw).is_err() {
            if !stop.load(Ordering::Relaxed) {
                let _ = tx.send(Msg::VizError(
                    "audio capture stopped (check `visualizer.source`, see: pactl list short sources)".to_string(),
                ));
            }
            break;
        }
        samples.copy_within(HOP.., 0);
        for i in 0..HOP {
            let s = i16::from_le_bytes([raw[2 * i], raw[2 * i + 1]]) as f32 / 32768.0;
            samples[N - HOP + i] = s;
        }
        if last_sent.elapsed() < frame {
            continue;
        }
        last_sent = Instant::now();

        let mut buf: Vec<Complex<f32>> = samples
            .iter()
            .zip(window.iter())
            .map(|(s, w)| Complex { re: s * w, im: 0.0 })
            .collect();
        fft.process(&mut buf);
        let mags: Vec<f32> = buf[..N / 2].iter().map(|c| c.norm()).collect();

        for b in 0..bars {
            let lo = f_lo * (f_hi / f_lo).powf(b as f32 / bars as f32);
            let hi = f_lo * (f_hi / f_lo).powf((b + 1) as f32 / bars as f32);
            let i0 = ((lo / bin_hz) as usize).clamp(1, N / 2 - 2);
            let i1 = ((hi / bin_hz).ceil() as usize).clamp(i0 + 1, N / 2 - 1);
            let peak = mags[i0..i1].iter().cloned().fold(0.0f32, f32::max);
            // normalise (full-scale sine ~ N/4), convert to dB, add a gentle treble tilt
            let db = 20.0 * (peak * gain / (N as f32 / 4.0) + 1e-9).log10()
                + 10.0 * (b as f32 / bars as f32);
            let v = ((db + 60.0) / 60.0).clamp(0.0, 1.0);
            smooth[b] = if v > smooth[b] {
                v
            } else {
                smooth[b] * decay + v * (1.0 - decay)
            };
        }
        if tx.send(Msg::Viz(smooth.clone())).is_err() {
            break;
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}
