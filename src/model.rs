//! Header-SNR -> tile-block success model per level: p = (1 - lapse) / (1 + exp(-(snr - mid)/scale)).
//! The default (calibrated in the simulator from the measured room) is compiled in; `calibrate`
//! re-fits it.

use crate::channel::{AcousticChannel, Interferer};
use crate::frame::Header;
use crate::link::{build_frame, receive_frame};
use crate::phy::{Phy, MAX_LEVEL};
use rand::rngs::StdRng;
use rand::{Rng, RngCore, SeedableRng};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

const DEFAULT: &str = include_str!("../ofdm/data/snr_model.json");

#[derive(Serialize, Deserialize, Clone, Copy, Debug)]
pub struct LevelModel {
    pub mid: f64,
    pub scale: f64,
    #[serde(default)]
    pub lapse: f64,
    pub thr90: f64,
    #[serde(default)]
    pub n: usize,
}

#[derive(Clone, Debug)]
pub struct SnrModel(pub [LevelModel; MAX_LEVEL as usize + 1]);

impl SnrModel {
    pub fn from_json(s: &str) -> anyhow::Result<Self> {
        let m: BTreeMap<String, LevelModel> = serde_json::from_str(s)?;
        let none = LevelModel { mid: 0.0, scale: 1.0, lapse: 0.0, thr90: f64::NEG_INFINITY, n: 0 };
        let get = |l: u8| m.get(&l.to_string()).copied().ok_or_else(|| anyhow::anyhow!("level {l} missing"));
        Ok(Self([none, get(1)?, get(2)?, get(3)?]))
    }

    pub fn default_model() -> Self {
        Self::from_json(DEFAULT).expect("embedded SNR model")
    }

    pub fn to_json(&self) -> String {
        let m: BTreeMap<String, LevelModel> = (1..=MAX_LEVEL).map(|l| (l.to_string(), self.0[l as usize])).collect();
        serde_json::to_string_pretty(&m).unwrap()
    }

    /// Success probability at `level` for a header SNR (NaN counts as -10 dB).
    pub fn p(&self, level: u8, snr_db: f64) -> f64 {
        if level == 0 {
            return 1.0;
        }
        let m = &self.0[level as usize];
        let s = if snr_db.is_nan() { -10.0 } else { snr_db };
        let z = ((s - m.mid) / m.scale).clamp(-30.0, 30.0);
        (1.0 - m.lapse) / (1.0 + (-z).exp())
    }
}

/// Collect (snr, level, ok) tile samples from simulated frames with random interference.
pub fn collect(phy: &Phy, frames_per_level: usize, seed: u64) -> anyhow::Result<Vec<(f64, u8, bool)>> {
    let mut rng = StdRng::seed_from_u64(seed);
    let fs = phy.cfg.fs as f64;
    let mut rows = vec![];
    for name in ["dell-g5_to_e540", "e540_to_dell-g5"] {
        let mut ch = AcousticChannel::new(name, seed)?;
        for lev in 1..=MAX_LEVEL {
            for i in 0..frames_per_level {
                let lo = rng.gen_range(800.0..8000.0);
                let hi = lo + rng.gen_range(500.0..4000.0);
                let its = if i % 3 != 0 { vec![Interferer::new(lo, hi, rng.gen_range(-10.0..15.0))] } else { vec![] };
                let levels = vec![lev; phy.cfg.n_tiles()];
                let (wave, pl) = build_frame(phy, &Header::new(0, levels.clone()), &levels, |n| {
                    let mut b = vec![0u8; n];
                    rng.fill_bytes(&mut b);
                    b
                });
                let r = receive_frame(phy, &ch.apply(&wave, fs, &its), Some(&levels), Some(&pl));
                rows.extend(r.tile_snr_db.iter().zip(&r.tile_ok).filter(|(s, _)| s.is_finite()).map(|(&s, &o)| (s, lev, o)));
            }
        }
    }
    Ok(rows)
}

/// 3-parameter maximum likelihood (mid, scale, lapse) per level by grid search.
pub fn fit(rows: &[(f64, u8, bool)]) -> SnrModel {
    let mut m = SnrModel::default_model();
    for lev in 1..=MAX_LEVEL {
        let d: Vec<(f64, f64)> = rows.iter().filter(|r| r.1 == lev).map(|r| (r.0, r.2 as u8 as f64)).collect();
        let mut best = (f64::INFINITY, 0.0, 1.0, 0.0);
        let mut mid = 0.0;
        while mid < 35.0 {
            let mut sc = 0.3;
            while sc < 5.0 {
                for lapse in [0.0, 0.01, 0.02, 0.05, 0.1] {
                    let nll: f64 = d
                        .iter()
                        .map(|&(s, y)| {
                            let q = (1.0 - lapse) / (1.0 + (-((s - mid) / sc).clamp(-30.0, 30.0)).exp());
                            -(y * (q + 1e-9).ln() + (1.0 - y) * (1.0 - q + 1e-9).ln())
                        })
                        .sum();
                    if nll < best.0 {
                        best = (nll, mid, sc, lapse);
                    }
                }
                sc += 0.1;
            }
            mid += 0.2;
        }
        let (_, mid, scale, lapse) = best;
        m.0[lev as usize] = LevelModel { mid, scale, lapse, thr90: mid + scale * 9f64.ln(), n: d.len() };
    }
    m
}
