//! Link-adaptation strategies: choose per-tile levels for the next frame from receiver feedback
//! (levels used, per-tile block success, per-tile header SNR measured on every tile).
//!
//! - Fixed: one level everywhere
//! - SnrThreshold: highest level whose 90%-success SNR (+ margin) is met (ADSL-like)
//! - SnrExpected: level with the best expected throughput under the SNR model
//! - Olla: SnrExpected with a per-tile outer-loop margin driven by block outcomes (target BLER)
//! - Minstrel: EWMA success per tile and level, ~10% random lookaround (Linux Wi-Fi rate control)
//! - ThompsonSw: Thompson sampling with discounted Beta counts and the SNR model as prior
//!   (the chosen strategy: best in simulation, +11% over fixed D8PSK over the air)

use crate::frame::payload_capacity;
use crate::model::SnrModel;
use crate::phy::{PhyConfig, MAX_LEVEL};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use rand_distr::{Beta, Distribution};

const NL: usize = MAX_LEVEL as usize + 1;

pub trait Controller: Send {
    fn name(&self) -> String;
    /// Levels for the next frame.
    fn choose(&mut self) -> Vec<u8>;
    /// Feedback for the last frame (snr may be NaN if the frame was lost).
    fn observe(&mut self, levels: &[u8], ok: &[bool], snr_db: &[f64]);
}

fn caps(cfg: &PhyConfig) -> [f64; NL] {
    let mut c = [0.0; NL];
    for l in 0..NL {
        c[l] = payload_capacity(cfg, l as u8) as f64;
    }
    c
}

fn argmax(v: &[f64]) -> usize {
    v.iter().enumerate().fold(0, |b, (i, &x)| if x > v[b] { i } else { b })
}

pub struct Fixed {
    pub level: u8,
    n: usize,
}

impl Fixed {
    pub fn new(cfg: &PhyConfig, level: u8) -> Self {
        Self { level, n: cfg.n_tiles() }
    }
}

impl Controller for Fixed {
    fn name(&self) -> String {
        format!("fixed-L{}", self.level)
    }
    fn choose(&mut self) -> Vec<u8> {
        vec![self.level; self.n]
    }
    fn observe(&mut self, _: &[u8], _: &[bool], _: &[f64]) {}
}

/// Shared SNR tracking (EWMA of the header SNR; a lost frame counts as 0 dB) and optional online
/// re-calibration of the model (per-level SNR offset learnt from outcomes).
pub struct SnrTracker {
    pub model: SnrModel,
    pub snr: Vec<f64>,
    alpha: f64,
    lost_db: f64,
    pub online_cal: bool,
    cal_lr: f64,
    pub offset: [f64; NL],
}

impl SnrTracker {
    pub fn new(n: usize, model: SnrModel, online_cal: bool) -> Self {
        Self { model, snr: vec![f64::NAN; n], alpha: 0.6, lost_db: 0.0, online_cal, cal_lr: 0.5, offset: [0.0; NL] }
    }

    pub fn known(&self) -> bool {
        self.snr.iter().any(|s| s.is_finite())
    }

    pub fn track(&mut self, snr_db: &[f64]) {
        for (cur, &s) in self.snr.iter_mut().zip(snr_db) {
            let s = if s.is_finite() { s } else { self.lost_db };
            *cur = if cur.is_finite() { self.alpha * s + (1.0 - self.alpha) * *cur } else { s };
        }
    }

    /// p[tile][level] with the tile SNR reduced by `margin[tile]`.
    pub fn p(&self, margin: &[f64]) -> Vec<[f64; NL]> {
        self.snr
            .iter()
            .zip(margin)
            .map(|(&s, &m)| {
                let mut p = [1.0; NL];
                for l in 1..NL {
                    p[l] = self.model.p(l as u8, if s.is_nan() { f64::NAN } else { s + self.offset[l] - m });
                }
                p
            })
            .collect()
    }

    pub fn calibrate(&mut self, levels: &[u8], ok: &[bool]) {
        if !self.online_cal || !self.known() {
            return;
        }
        let p = self.p(&vec![0.0; self.snr.len()]);
        for l in 1..NL {
            let used: Vec<usize> = (0..levels.len()).filter(|&k| levels[k] as usize == l).collect();
            if used.len() >= 4 {
                let err = used.iter().map(|&k| ok[k] as u8 as f64 - p[k][l]).sum::<f64>() / used.len() as f64;
                self.offset[l] = (self.offset[l] + self.cal_lr * 4.0 * self.model.0[l].scale * err).clamp(-8.0, 8.0);
            }
        }
    }
}

pub struct SnrThreshold {
    t: SnrTracker,
    margin: f64,
}

impl SnrThreshold {
    pub fn new(cfg: &PhyConfig, margin: f64) -> Self {
        Self { t: SnrTracker::new(cfg.n_tiles(), SnrModel::default_model(), false), margin }
    }
}

impl Controller for SnrThreshold {
    fn name(&self) -> String {
        format!("snr-thr+{}dB", self.margin)
    }
    fn choose(&mut self) -> Vec<u8> {
        if !self.t.known() {
            return vec![1; self.t.snr.len()];
        }
        self.t
            .snr
            .iter()
            .map(|&s| {
                let s = if s.is_nan() { -10.0 } else { s };
                (1..NL).filter(|&l| s - self.margin >= self.t.model.0[l].thr90).max().unwrap_or(0) as u8
            })
            .collect()
    }
    fn observe(&mut self, _: &[u8], _: &[bool], snr: &[f64]) {
        self.t.track(snr);
    }
}

pub struct SnrExpected {
    t: SnrTracker,
    cap: [f64; NL],
}

impl SnrExpected {
    pub fn new(cfg: &PhyConfig, online_cal: bool) -> Self {
        Self { t: SnrTracker::new(cfg.n_tiles(), SnrModel::default_model(), online_cal), cap: caps(cfg) }
    }
}

impl Controller for SnrExpected {
    fn name(&self) -> String {
        format!("snr-expected{}", if self.t.online_cal { "+cal" } else { "" })
    }
    fn choose(&mut self) -> Vec<u8> {
        if !self.t.known() {
            return vec![1; self.t.snr.len()];
        }
        let n = self.t.snr.len();
        self.t.p(&vec![0.0; n]).iter().map(|p| argmax(&(0..NL).map(|l| p[l] * self.cap[l]).collect::<Vec<_>>()) as u8).collect()
    }
    fn observe(&mut self, levels: &[u8], ok: &[bool], snr: &[f64]) {
        self.t.calibrate(levels, ok);
        self.t.track(snr);
    }
}

pub struct Olla {
    t: SnrTracker,
    cap: [f64; NL],
    margin: Vec<f64>,
    up: f64,
    down: f64,
    target: f64,
}

impl Olla {
    pub fn new(cfg: &PhyConfig, target_bler: f64, step_db: f64) -> Self {
        let n = cfg.n_tiles();
        Self {
            t: SnrTracker::new(n, SnrModel::default_model(), false),
            cap: caps(cfg),
            margin: vec![0.0; n],
            up: step_db,
            down: step_db * target_bler / (1.0 - target_bler),
            target: target_bler,
        }
    }
}

impl Controller for Olla {
    fn name(&self) -> String {
        format!("olla-{}%", (self.target * 100.0).round())
    }
    fn choose(&mut self) -> Vec<u8> {
        if !self.t.known() {
            return vec![1; self.t.snr.len()];
        }
        self.t.p(&self.margin).iter().map(|p| argmax(&(0..NL).map(|l| p[l] * self.cap[l]).collect::<Vec<_>>()) as u8).collect()
    }
    fn observe(&mut self, levels: &[u8], ok: &[bool], snr: &[f64]) {
        self.t.track(snr);
        for k in 0..levels.len() {
            if levels[k] > 0 {
                self.margin[k] = (self.margin[k] + if ok[k] { -self.down } else { self.up }).clamp(-3.0, 15.0);
            }
        }
    }
}

pub struct Minstrel {
    p: Vec<[f64; NL]>,
    cap: [f64; NL],
    w: f64,
    look: f64,
    rng: StdRng,
}

impl Minstrel {
    pub fn new(cfg: &PhyConfig, seed: u64) -> Self {
        Self { p: vec![[1.0, 0.9, 0.7, 0.5]; cfg.n_tiles()], cap: caps(cfg), w: 0.75, look: 0.1, rng: StdRng::seed_from_u64(seed) }
    }
}

impl Controller for Minstrel {
    fn name(&self) -> String {
        "minstrel".into()
    }
    fn choose(&mut self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.p.len());
        for p in &self.p {
            let best = argmax(&(0..NL).map(|l| p[l] * self.cap[l]).collect::<Vec<_>>()) as u8;
            out.push(if self.rng.gen::<f64>() < self.look { self.rng.gen_range(1..=MAX_LEVEL) } else { best });
        }
        out
    }
    fn observe(&mut self, levels: &[u8], ok: &[bool], _: &[f64]) {
        for k in 0..levels.len() {
            let l = levels[k] as usize;
            if l > 0 {
                self.p[k][l] = self.w * self.p[k][l] + (1.0 - self.w) * ok[k] as u8 as f64;
            }
        }
    }
}

/// Thompson sampling per tile over levels with discounted Beta(success, failure) counts; the SNR
/// model adds `prior_weight` pseudo-observations per frame (fresh header SNR + outcome statistics).
/// Sampled success probabilities are made non-increasing in the level.
pub struct ThompsonSw {
    t: SnrTracker,
    cap: [f64; NL],
    s: Vec<[f64; NL]>,
    f: Vec<[f64; NL]>,
    gamma: f64,
    snr_prior: bool,
    prior_weight: f64,
    /// > 1 prefers reliable levels: maximise cap * p^risk. A packet spanning k tiles needs all of
    /// > them, so for IP traffic (risk 2) tile reliability is worth more than raw tile throughput.
    pub risk: f64,
    rng: StdRng,
}

impl ThompsonSw {
    pub fn new(cfg: &PhyConfig, gamma: f64, snr_prior: bool, online_cal: bool, seed: u64) -> Self {
        let n = cfg.n_tiles();
        Self {
            t: SnrTracker::new(n, SnrModel::default_model(), online_cal),
            cap: caps(cfg),
            s: vec![[0.0; NL]; n],
            f: vec![[0.0; NL]; n],
            gamma,
            snr_prior,
            prior_weight: 3.0,
            risk: 1.0,
            rng: StdRng::seed_from_u64(seed),
        }
    }

    /// The link's default controller (file transfer: risk-neutral).
    pub fn default_for(cfg: &PhyConfig, seed: u64) -> Self {
        Self::new(cfg, 0.7, true, false, seed)
    }

    /// For IP packets spanning several tiles: risk-averse.
    pub fn for_packets(cfg: &PhyConfig, seed: u64) -> Self {
        Self { risk: 2.0, ..Self::new(cfg, 0.7, true, false, seed) }
    }
}

impl Controller for ThompsonSw {
    fn name(&self) -> String {
        let risk = if self.risk != 1.0 { format!("-r{}", self.risk) } else { String::new() };
        format!("thompson{}{}-g{}{risk}", if self.snr_prior { "+snr" } else { "" }, if self.t.online_cal { "+cal" } else { "" }, self.gamma)
    }
    fn choose(&mut self) -> Vec<u8> {
        let n = self.s.len();
        let prior = self.snr_prior && self.t.known();
        let p0 = if prior { self.t.p(&vec![0.0; n]) } else { vec![[1.0; NL]; n] };
        (0..n)
            .map(|k| {
                let mut theta = [1.0; NL];
                for l in 1..NL {
                    let (mut a, mut b) = (1.0 + self.s[k][l], 1.0 + self.f[k][l]);
                    if prior {
                        a += self.prior_weight * p0[k][l];
                        b += self.prior_weight * (1.0 - p0[k][l]);
                    }
                    theta[l] = Beta::new(a, b).map(|d| d.sample(&mut self.rng)).unwrap_or(0.5);
                }
                for l in 1..NL {
                    theta[l] = theta[l].min(theta[l - 1]);
                }
                argmax(&(0..NL).map(|l| theta[l].powf(self.risk) * self.cap[l]).collect::<Vec<_>>()) as u8
            })
            .collect()
    }
    fn observe(&mut self, levels: &[u8], ok: &[bool], snr: &[f64]) {
        self.t.calibrate(levels, ok);
        self.t.track(snr);
        for k in 0..levels.len() {
            for l in 0..NL {
                self.s[k][l] *= self.gamma;
                self.f[k][l] *= self.gamma;
            }
            let l = levels[k] as usize;
            if l > 0 {
                if ok[k] { self.s[k][l] += 1.0 } else { self.f[k][l] += 1.0 }
            }
        }
    }
}

/// Every strategy, for comparisons.
pub fn all_strategies(cfg: &PhyConfig, seed: u64) -> Vec<Box<dyn Controller>> {
    vec![
        Box::new(Fixed::new(cfg, 2)),
        Box::new(Fixed::new(cfg, 3)),
        Box::new(SnrThreshold::new(cfg, 0.0)),
        Box::new(SnrThreshold::new(cfg, 3.0)),
        Box::new(Olla::new(cfg, 0.1, 2.0)),
        Box::new(Minstrel::new(cfg, seed)),
        Box::new(SnrExpected::new(cfg, false)),
        Box::new(ThompsonSw::new(cfg, 0.7, false, false, seed)),
        Box::new(ThompsonSw::new(cfg, 0.7, true, false, seed)),
        Box::new(ThompsonSw::new(cfg, 0.5, true, false, seed)),
        Box::new(ThompsonSw::new(cfg, 0.85, true, false, seed)),
        Box::new(ThompsonSw::new(cfg, 0.7, true, true, seed)),
    ]
}

/// The over-the-air finalists.
pub fn finalists(cfg: &PhyConfig, seed: u64) -> Vec<Box<dyn Controller>> {
    vec![
        Box::new(Fixed::new(cfg, 3)),
        Box::new(ThompsonSw::new(cfg, 0.7, true, false, seed)),
        Box::new(ThompsonSw::new(cfg, 0.7, true, true, seed)),
        Box::new(ThompsonSw::for_packets(cfg, seed)),
    ]
}
