//! Browser bindings for the `dos` adaptive OFDM modem.
//!
//! - [`station`]: one device running the `dos` leader/follower link with a file transfer
//! - [`classic`]: the original `dos modem` transfer (minimodem-compatible FSK + Stop-and-Wait)
//! - [`sim`]: the measured room between two stations, streamed in real time
//! - the `wasm_bindgen` surface below, used by `web/src/lib/worker.ts`

pub mod classic;
pub mod sim;
pub mod station;

use dos::channel::ChannelData;
use dos::frame::payload_capacity;
use dos::phy::{PhyConfig, MAX_LEVEL};
use serde::Serialize;
use station::{config, validate, wrap_envelope, Delivered, Role, Station as Core};
use wasm_bindgen::prelude::*;

fn err(e: impl std::fmt::Display) -> JsError {
    JsError::new(&e.to_string())
}

#[derive(Serialize)]
struct BandInfo {
    f_lo: f64,
    f_hi: f64,
    /// the band `dos` uses: same pseudo-random tables, talks to the `dos` binary
    dos_default: bool,
    n_tiles: usize,
    max_tiles: usize,
    tile_hz: f64,
    tile_freqs: Vec<f64>,
    frame_s: f64,
    /// payload bytes per tile at levels 0..=3
    tile_payload: Vec<usize>,
    /// payload bytes per frame: every tile robust (L1) / every tile D8PSK (L3)
    frame_payload_robust: usize,
    frame_payload_best: usize,
}

/// Band layout and capacity, or an error explaining why the band can't work.
#[wasm_bindgen]
pub fn band_info(f_lo: f64, f_hi: f64) -> Result<String, JsError> {
    let cfg = config(f_lo, f_hi);
    validate(&cfg).map_err(err)?;
    let nt = cfg.n_tiles();
    let tile_payload: Vec<usize> = (0..=MAX_LEVEL).map(|l| payload_capacity(&cfg, l)).collect();
    let info = BandInfo {
        f_lo,
        f_hi,
        dos_default: cfg == PhyConfig::default(),
        n_tiles: nt,
        max_tiles: station::max_tiles(),
        tile_hz: cfg.tile_sc as f64 * cfg.df(),
        tile_freqs: (0..nt).map(|k| cfg.tile_freq(k)).collect(),
        frame_s: cfg.frame_s(),
        frame_payload_robust: tile_payload[1] * nt,
        frame_payload_best: tile_payload[3] * nt,
        tile_payload,
    };
    serde_json::to_string(&info).map_err(err)
}

#[derive(Serialize)]
struct Estimate {
    rounds_best: usize,
    rounds_robust: usize,
    seconds_best: f64,
    seconds_robust: f64,
}

/// Rough transfer time: one round is a leader frame and the follower's answer.
#[wasm_bindgen]
pub fn estimate(f_lo: f64, f_hi: f64, bytes: usize) -> Result<String, JsError> {
    let cfg = config(f_lo, f_hi);
    validate(&cfg).map_err(err)?;
    let nt = cfg.n_tiles();
    let per_tile = |l: u8| payload_capacity(&cfg, l).saturating_sub(8).max(1);
    let round_s = 2.0 * (cfg.frame_s() + 0.7 + 0.6);
    // the first frame goes out on the robust map; feedback upgrades it from then on
    let rounds = |l: u8| 1 + bytes.saturating_sub(per_tile(1) * nt).div_ceil(per_tile(l) * nt) + 1;
    let (best, robust) = (rounds(3), rounds(1));
    serde_json::to_string(&Estimate {
        rounds_best: best,
        rounds_robust: robust,
        seconds_best: best as f64 * round_s,
        seconds_robust: robust as f64 * round_s,
    })
    .map_err(err)
}

#[wasm_bindgen]
pub fn scenario_names() -> String {
    serde_json::to_string(&dos::scenario::phases().iter().map(|p| p.0).collect::<Vec<_>>()).unwrap_or_default()
}

#[wasm_bindgen]
pub struct Station(Core);

#[wasm_bindgen]
impl Station {
    /// `role`: "leader" (sends, drives the exchange) or "follower" (answers).
    #[wasm_bindgen(constructor)]
    pub fn new(f_lo: f64, f_hi: f64, role: &str, seed: u64) -> Result<Station, JsError> {
        let role = parse_role(role)?;
        Ok(Station(Core::new(config(f_lo, f_hi), role, None, seed).map_err(err)?))
    }

    /// A station that delivers a file (or message) to its peer.
    #[allow(clippy::too_many_arguments)]
    pub fn sending(f_lo: f64, f_hi: f64, role: &str, seed: u64, name: &str, mime: &str, flags: u8, data: &[u8]) -> Result<Station, JsError> {
        let role = parse_role(role)?;
        let stream = wrap_envelope(name, mime, flags, data);
        Ok(Station(Core::new(config(f_lo, f_hi), role, Some(stream), seed).map_err(err)?))
    }

    pub fn push_audio(&mut self, samples: &[f32]) {
        self.0.push_audio(samples);
    }

    pub fn poll(&mut self, now_ms: f64) -> Option<Vec<f32>> {
        self.0.poll(now_ms)
    }

    pub fn tx_done(&mut self, now_ms: f64) {
        self.0.tx_done(now_ms);
    }

    pub fn stop(&mut self) {
        self.0.stop();
    }

    pub fn status_json(&self) -> String {
        serde_json::to_string(&self.0.status()).unwrap_or_default()
    }

    pub fn take_completed(&mut self) -> Option<Completed> {
        self.0.take_completed().map(Completed)
    }
}

fn parse_role(role: &str) -> Result<Role, JsError> {
    match role {
        "leader" => Ok(Role::Leader),
        "follower" => Ok(Role::Follower),
        r => Err(err(format!("role must be leader or follower, not {r}"))),
    }
}

#[wasm_bindgen]
pub struct Completed(Delivered);

#[wasm_bindgen]
impl Completed {
    #[wasm_bindgen(getter)]
    pub fn name(&self) -> String {
        self.0.name.clone()
    }
    #[wasm_bindgen(getter)]
    pub fn mime(&self) -> String {
        self.0.mime.clone()
    }
    #[wasm_bindgen(getter)]
    pub fn flags(&self) -> u8 {
        self.0.flags
    }
    #[wasm_bindgen(getter)]
    pub fn data(&self) -> Vec<u8> {
        self.0.data.clone()
    }
}

#[wasm_bindgen]
pub struct Air(sim::Air);

#[wasm_bindgen]
impl Air {
    /// Channel JSON as in `data/channel_*.json` (`a_to_b`: what station 1 hears from station 0).
    #[wasm_bindgen(constructor)]
    pub fn new(a_to_b: &str, b_to_a: &str, seed: u64) -> Result<Air, JsError> {
        let ab: ChannelData = serde_json::from_str(a_to_b).map_err(err)?;
        let ba: ChannelData = serde_json::from_str(b_to_a).map_err(err)?;
        Ok(Air(sim::Air::new(ab, ba, 48_000, seed)))
    }

    /// Interference from `dos::scenario::phases()` by index; out of range = quiet.
    pub fn set_scenario(&mut self, index: usize) {
        self.0.interferers = dos::scenario::phases().get(index).map(|p| p.1.clone()).unwrap_or_default();
    }

    pub fn transmit(&mut self, from: usize, wave: &[f32]) {
        self.0.transmit(from.min(1), wave);
    }

    pub fn hear(&mut self, who: usize, n: usize) -> Vec<f32> {
        self.0.hear(who.min(1), n)
    }
}

/// Waterfall rows: dB power, max-pooled onto `bins` display columns over `[lo, hi]` Hz.
#[wasm_bindgen]
pub struct Spectrum {
    n: usize,
    buf: Vec<f64>,
    window: Vec<f64>,
    lo: f64,
    hi: f64,
    bins: usize,
}

#[wasm_bindgen]
impl Spectrum {
    #[wasm_bindgen(constructor)]
    pub fn new(lo: f64, hi: f64, bins: usize) -> Spectrum {
        let n = 2048;
        let window = (0..n).map(|i| 0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / n as f64).cos()).collect();
        Spectrum { n, buf: Vec::new(), window, lo, hi, bins: bins.max(8) }
    }

    pub fn push(&mut self, samples: &[f32]) -> Vec<f32> {
        self.buf.extend(samples.iter().map(|&v| v as f64));
        let hz = 48_000.0 / self.n as f64;
        let mut out = Vec::new();
        while self.buf.len() >= self.n {
            let seg: Vec<f64> = self.buf[..self.n].iter().zip(&self.window).map(|(v, w)| v * w).collect();
            let spec = dos::dsp::rfft(&seg);
            let norm = 4.0 / (self.n as f64 * self.n as f64);
            for i in 0..self.bins {
                let a = self.lo + (self.hi - self.lo) * i as f64 / self.bins as f64;
                let b = self.lo + (self.hi - self.lo) * (i + 1) as f64 / self.bins as f64;
                let ia = ((a / hz) as usize).min(spec.len() - 1);
                let ib = ((b / hz) as usize).clamp(ia + 1, spec.len());
                let p = spec[ia..ib].iter().map(|c| c.norm_sqr() * norm).fold(1e-14, f64::max);
                out.push((10.0 * p.log10()) as f32);
            }
            self.buf.drain(..self.n);
        }
        out
    }
}

/// The original `dos modem` transfer: 100 baud FSK on 2300/1900 Hz, Stop-and-Wait ARQ.
#[wasm_bindgen]
pub struct ClassicStation(classic::Classic);

#[wasm_bindgen]
impl ClassicStation {
    pub fn receiver() -> ClassicStation {
        ClassicStation(classic::Classic::receiver(classic::AfskConfig::default()))
    }

    /// Raw bytes, exactly what `dos modem recv out.bin` writes on the other side.
    pub fn sender(data: &[u8]) -> ClassicStation {
        ClassicStation(classic::Classic::sender(classic::AfskConfig::default(), data))
    }

    pub fn push_audio(&mut self, samples: &[f32], now_ms: f64) {
        self.0.push_audio(samples, now_ms);
    }

    pub fn poll(&mut self, now_ms: f64) -> Option<Vec<f32>> {
        self.0.poll(now_ms)
    }

    pub fn tx_done(&mut self, now_ms: f64) {
        self.0.tx_done(now_ms);
    }

    pub fn stop(&mut self) {
        self.0.stop();
    }

    pub fn status_json(&self) -> String {
        serde_json::to_string(&self.0.status()).unwrap_or_default()
    }

    pub fn take_completed(&mut self) -> Option<Vec<u8>> {
        self.0.take_completed()
    }
}
