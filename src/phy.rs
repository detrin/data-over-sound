//! OFDM physical layer for an acoustic (speaker -> room -> mic) channel.
//!
//! Frame: [sync chirp][gap][reference symbol][header symbols][data symbols]
//! - Long symbols (640 ms + 160 ms cyclic prefix) turn most of the room reverb into part of the
//!   channel instead of interference.
//! - Differential PSK in time on every subcarrier: no equaliser needed.
//! - The band is split into tiles; each tile has a level: 0 = pilot (known DQPSK, measured only),
//!   1 = DBPSK x3 repetition (soft-combined), 2 = DQPSK, 3 = D8PSK.
//! - Every tile's effective SNR is measured from the phase error of the differential products;
//!   the DBPSK header symbol gives an unfolded estimate for every tile in every frame.

use crate::dsp;
use crate::wire::WireTables;
use num_complex::Complex64;
use std::f64::consts::PI;

pub const MAX_LEVEL: u8 = 3;
pub const L1_REPEAT: usize = 3;
const GRAY: [&[usize]; 4] = [&[0], &[0, 1], &[0, 1, 3, 2], &[0, 1, 3, 2, 6, 7, 5, 4]];

#[derive(Clone, Debug, PartialEq)]
pub struct PhyConfig {
    pub fs: usize,
    pub nfft: usize,
    pub ncp: usize,
    pub f_lo: f64,
    pub f_hi: f64,
    pub tile_sc: usize,
    pub n_header: usize,
    pub n_data: usize,
    pub chirp_s: f64,
    pub gap_s: f64,
    pub tx_rms: f64,
    pub tx_clip: f64,
    pub rs_fraction: f64,
}

impl Default for PhyConfig {
    /// Chosen on the measured room (reverb only ~10 dB down after 60 ms): 640 ms symbols,
    /// 160 ms cyclic prefix, 800-10000 Hz, 92 tiles of 100 Hz.
    fn default() -> Self {
        Self {
            fs: 48000,
            nfft: 30720,
            ncp: 7680,
            f_lo: 800.0,
            f_hi: 10000.0,
            tile_sc: 64,
            n_header: 1,
            n_data: 6,
            chirp_s: 0.25,
            gap_s: 0.15,
            tx_rms: 0.12,
            tx_clip: 0.6,
            rs_fraction: 0.25,
        }
    }
}

impl PhyConfig {
    pub fn df(&self) -> f64 {
        self.fs as f64 / self.nfft as f64
    }
    pub fn sym_len(&self) -> usize {
        self.nfft + self.ncp
    }
    pub fn first_bin(&self) -> usize {
        (self.f_lo / self.df()).ceil() as usize
    }
    pub fn n_bins(&self) -> usize {
        let hi = (self.f_hi / self.df()) as usize;
        (hi - self.first_bin()) / self.tile_sc * self.tile_sc
    }
    pub fn n_tiles(&self) -> usize {
        self.n_bins() / self.tile_sc
    }
    pub fn n_syms(&self) -> usize {
        1 + self.n_header + self.n_data
    }
    pub fn sync_len(&self) -> usize {
        (self.chirp_s * self.fs as f64) as usize
    }
    pub fn gap_len(&self) -> usize {
        (self.gap_s * self.fs as f64) as usize
    }
    /// Samples in one frame.
    pub fn frame_len(&self) -> usize {
        self.sync_len() + self.gap_len() + self.n_syms() * self.sym_len()
    }
    pub fn frame_s(&self) -> f64 {
        self.frame_len() as f64 / self.fs as f64
    }
    pub fn tile_bits(&self, level: u8) -> usize {
        match level {
            0 => 0,
            1 => self.tile_sc * self.n_data / L1_REPEAT,
            l => l as usize * self.tile_sc * self.n_data,
        }
    }
    pub fn header_bits(&self) -> usize {
        self.n_bins() * self.n_header
    }
    /// Header RS codeword length (one RS(255) codeword at most).
    pub fn header_bytes(&self) -> usize {
        (self.header_bits() / 8).min(255)
    }
    /// Copies of the header codeword on the header symbols (soft-combined at the receiver).
    pub fn header_copies(&self) -> usize {
        ((self.header_bits() / 8) / self.header_bytes()).max(1)
    }
    pub fn tile_freq(&self, k: usize) -> f64 {
        (self.first_bin() as f64 + (k * self.tile_sc) as f64 + (self.tile_sc as f64 - 1.0) / 2.0) * self.df()
    }
}

/// Demodulated frame.
pub struct Demod {
    /// soft DBPSK metrics of all header subcarriers (> 0 means bit 0), reliability weighted
    pub header_soft: Vec<f64>,
    pub tile_bits: Vec<Vec<u8>>,
    /// per-tile SNR from the data symbols (folds at high order)
    pub tile_snr_data_db: Vec<f64>,
    /// per-tile SNR from the DBPSK header symbol (every tile, unfolded)
    pub tile_snr_db: Vec<f64>,
    pub drift_ppm: f64,
}

/// Precomputed modem context for one configuration.
pub struct Phy {
    pub cfg: PhyConfig,
    pub sync: Vec<f64>,
    sync_energy: f64,
    pub tables: WireTables,
    bins: Vec<usize>,
    freqs: Vec<f64>,
}

fn wrap(a: f64) -> f64 {
    (a + PI).rem_euclid(2.0 * PI) - PI
}

impl Phy {
    pub fn new(cfg: PhyConfig) -> Self {
        let n = cfg.sync_len();
        let sync: Vec<f64> = dsp::chirp(n, cfg.fs as f64, cfg.f_lo, cfg.f_hi)
            .iter()
            .zip(dsp::hanning(n))
            .map(|(c, w)| c * w)
            .collect();
        let sync_energy = sync.iter().map(|v| v * v).sum();
        let bins: Vec<usize> = (cfg.first_bin()..cfg.first_bin() + cfg.n_bins()).collect();
        let freqs = bins.iter().map(|&b| b as f64 * cfg.df()).collect();
        let tables = WireTables::new(cfg.n_bins(), cfg.n_data, cfg.header_bits() / 8);
        Self { cfg, sync, sync_energy, tables, bins, freqs }
    }

    fn l1_positions(&self) -> Vec<Vec<usize>> {
        let n = self.cfg.tile_bits(1);
        (0..L1_REPEAT).map(|r| (0..n).map(|i| i + r * n).collect()).collect()
    }

    /// Build one frame. `header_bits`: cfg.header_bits() bits; `levels`: per tile;
    /// `tile_bits[k]`: cfg.tile_bits(levels[k]) bits (ignored for pilots).
    pub fn modulate(&self, header_bits: &[u8], levels: &[u8], tile_bits: &[Vec<u8>]) -> Vec<f64> {
        let c = &self.cfg;
        let (nb, ts, nh) = (c.n_bins(), c.tile_sc, c.n_header);
        let rows = c.n_syms() - 1;
        let mut steps = vec![vec![0.0f64; nb]; rows];
        for s in 0..nh {
            for b in 0..nb {
                steps[s][b] = PI * header_bits[s * nb + b] as f64;
            }
        }
        let l1pos = self.l1_positions();
        for (k, &lev) in levels.iter().enumerate() {
            let col0 = k * ts;
            match lev {
                0 => {
                    for s in 0..c.n_data {
                        for j in 0..ts {
                            steps[nh + s][col0 + j] = PI / 2.0 * self.tables.pilots[s][col0 + j] as f64;
                        }
                    }
                }
                1 => {
                    let mut grid = vec![0.0; c.n_data * ts];
                    for copy in &l1pos {
                        for (i, &p) in copy.iter().enumerate() {
                            grid[p] = PI * tile_bits[k][i] as f64;
                        }
                    }
                    for s in 0..c.n_data {
                        for j in 0..ts {
                            steps[nh + s][col0 + j] = grid[s * ts + j];
                        }
                    }
                }
                l => {
                    let lv = l as usize;
                    let m = 1usize << lv;
                    let bits = &tile_bits[k];
                    for g in 0..c.n_data * ts {
                        let idx = (0..lv).fold(0usize, |a, i| (a << 1) | bits[g * lv + i] as usize);
                        steps[nh + g / ts][col0 + g % ts] = 2.0 * PI / m as f64 * GRAY[lv][idx] as f64;
                    }
                }
            }
        }
        let half = c.nfft / 2 + 1;
        let mut body = Vec::with_capacity(c.n_syms() * c.sym_len());
        let mut phase: Vec<f64> = self.tables.reference.iter().map(|&r| PI / 2.0 * r as f64 + PI / 4.0).collect();
        for s in 0..c.n_syms() {
            if s > 0 {
                for b in 0..nb {
                    phase[b] += steps[s - 1][b];
                }
            }
            let mut spec = vec![Complex64::new(0.0, 0.0); half];
            for (b, &bin) in self.bins.iter().enumerate() {
                spec[bin] = Complex64::from_polar(1.0, phase[b]);
            }
            let x = dsp::irfft(&spec, c.nfft);
            body.extend_from_slice(&x[c.nfft - c.ncp..]);
            body.extend_from_slice(&x);
        }
        let scale = c.tx_rms / dsp::rms(&body);
        let mut out: Vec<f64> = self.sync.iter().map(|v| v * c.tx_rms * 3.0).collect();
        out.extend(std::iter::repeat_n(0.0, c.gap_len()));
        out.extend(body.iter().map(|v| c.tx_clip * (v * scale / c.tx_clip).tanh()));
        out
    }

    /// Index of the first sample after the sync chirp (direct path), or None.
    pub fn find_sync(&self, rx: &[f64]) -> Option<usize> {
        let c = dsp::xcorr_valid_abs(rx, &self.sync);
        if c.is_empty() {
            return None;
        }
        let peak = c.iter().enumerate().fold(0, |best, (i, &v)| if v > c[best] { i } else { best });
        let win = &rx[peak..(peak + self.sync.len()).min(rx.len())];
        let e: f64 = win.iter().map(|v| v * v).sum();
        let ncorr = c[peak] / (e * self.sync_energy).max(1e-30).sqrt();
        // room noise alone reaches max/median ~7-8 and ncorr <= 0.01; real frames >= 16 and >= 0.09
        if c[peak] < 12.0 * dsp::median(&c) || ncorr < 0.05 {
            return None;
        }
        let lo = peak.saturating_sub((0.03 * self.cfg.fs as f64) as usize);
        let early = lo + (lo..=peak).position(|i| c[i] > 0.5 * c[peak]).unwrap_or(peak - lo);
        Some(early + self.sync.len())
    }

    /// Demodulate the frame whose sync ends at `start` (or is searched for).
    pub fn demodulate(&self, rx: &[f64], levels: &[u8], start: Option<usize>) -> Option<Demod> {
        let c = &self.cfg;
        let start = match start {
            Some(s) => s,
            None => self.find_sync(rx)?,
        };
        let (nb, ts, nh, nt) = (c.n_bins(), c.tile_sc, c.n_header, c.n_tiles());
        let s0 = start + c.gap_len() + c.ncp - c.ncp / 8; // FFT windows slightly early
        let mut y: Vec<Vec<Complex64>> = Vec::with_capacity(c.n_syms());
        let mut seg = vec![0.0; c.nfft];
        for s in 0..c.n_syms() {
            let o = s0 + s * c.sym_len();
            for (i, v) in seg.iter_mut().enumerate() {
                *v = *rx.get(o + i).unwrap_or(&0.0);
            }
            let spec = dsp::rfft(&seg);
            y.push(self.bins.iter().map(|&b| spec[b]).collect());
        }
        let rows = c.n_syms() - 1;
        let z: Vec<Vec<Complex64>> = (0..rows).map(|s| (0..nb).map(|b| y[s + 1][b] * y[s][b].conj()).collect()).collect();
        let order: Vec<u8> = (0..nb).map(|b| levels[b / ts]).collect();

        let decide = |ang: &Vec<Vec<f64>>| -> (Vec<Vec<usize>>, Vec<Vec<f64>>) {
            let mut steps = vec![vec![0usize; nb]; rows];
            let mut res = vec![vec![0.0f64; nb]; rows];
            for s in 0..rows {
                for b in 0..nb {
                    let a = ang[s][b];
                    if s < nh {
                        let q = PI * (a / PI).round_ties_even();
                        steps[s][b] = ((q / PI).round_ties_even() as i64).rem_euclid(2) as usize;
                        res[s][b] = wrap(a - q);
                    } else {
                        let lev = order[b];
                        if lev == 0 {
                            let p = self.tables.pilots[s - nh][b] as usize;
                            steps[s][b] = p;
                            res[s][b] = wrap(a - PI / 2.0 * p as f64);
                        } else {
                            let m = (1usize << lev) as f64;
                            let st = ((a / (2.0 * PI / m)).round_ties_even() as i64).rem_euclid(m as i64) as usize;
                            steps[s][b] = st;
                            res[s][b] = wrap(a - 2.0 * PI / m * st as f64);
                        }
                    }
                }
            }
            (steps, res)
        };

        let mut ang: Vec<Vec<f64>> = z.iter().map(|r| r.iter().map(|v| v.arg()).collect()).collect();
        let (_, res0) = decide(&ang);
        // sample-clock drift: the phase step grows linearly with frequency; fit it on confident points
        let (mut num, mut den) = (0.0, 0.0);
        for s in 0..rows {
            for b in 0..nb {
                if res0[s][b].abs() < PI / 8.0 {
                    let w = z[s][b].norm();
                    num += w * res0[s][b] * self.freqs[b];
                    den += w * self.freqs[b] * self.freqs[b];
                }
            }
        }
        let slope = num / den.max(1e-12);
        let drift_ppm = slope / (2.0 * PI * c.sym_len() as f64 / c.fs as f64) * 1e6;
        for s in 0..rows {
            for b in 0..nb {
                ang[s][b] -= slope * self.freqs[b];
            }
        }
        let (steps, res) = decide(&ang);

        // soft DBPSK metric weighted by the reliability (1/phase-error variance) of that symbol in that tile
        let mut soft = vec![vec![0.0f64; nb]; rows];
        for s in 0..rows {
            for k in 0..nt {
                let var = (k * ts..(k + 1) * ts).map(|b| res[s][b] * res[s][b]).sum::<f64>() / ts as f64;
                let w = 1.0 / var.max(1e-3);
                for b in k * ts..(k + 1) * ts {
                    let zc = z[s][b] * Complex64::from_polar(1.0, -slope * self.freqs[b]);
                    soft[s][b] = zc.re / zc.norm().max(1e-30) * w;
                }
            }
        }

        let l1pos = self.l1_positions();
        let mut tile_bits = Vec::with_capacity(nt);
        for (k, &lev) in levels.iter().enumerate() {
            let flat = |s: usize, j: usize| (nh + s, k * ts + j);
            match lev {
                0 => tile_bits.push(vec![]),
                1 => {
                    let n = c.tile_bits(1);
                    let mut bits = vec![0u8; n];
                    for i in 0..n {
                        let sum: f64 = l1pos.iter().map(|copy| {
                            let p = copy[i];
                            let (s, b) = flat(p / ts, p % ts);
                            soft[s][b]
                        }).sum();
                        bits[i] = (sum < 0.0) as u8;
                    }
                    tile_bits.push(bits);
                }
                l => {
                    let lv = l as usize;
                    let inv: Vec<usize> = {
                        let mut inv = vec![0; 1 << lv];
                        for (i, &g) in GRAY[lv].iter().enumerate() {
                            inv[g] = i;
                        }
                        inv
                    };
                    let mut bits = Vec::with_capacity(c.n_data * ts * lv);
                    for g in 0..c.n_data * ts {
                        let (s, b) = flat(g / ts, g % ts);
                        let idx = inv[steps[s][b]];
                        for i in (0..lv).rev() {
                            bits.push(((idx >> i) & 1) as u8);
                        }
                    }
                    tile_bits.push(bits);
                }
            }
        }

        // tiles with (almost) no energy must not look clean: a zero signal has zero phase error
        let p: Vec<f64> = (0..nt)
            .map(|k| {
                (0..c.n_syms()).map(|s| (k * ts..(k + 1) * ts).map(|b| y[s][b].norm_sqr()).sum::<f64>()).sum::<f64>()
                    / (c.n_syms() * ts) as f64
            })
            .collect();
        let med = dsp::median(&p);
        let any_signal = p.iter().any(|&v| v > 1e-20);
        let dead: Vec<bool> = p.iter().map(|&v| !any_signal || v < 1e-3 * med.max(1e-30)).collect();
        let tile_snr = |r0: usize, r1: usize| -> Vec<f64> {
            (0..nt)
                .map(|k| {
                    if dead[k] {
                        return -10.0;
                    }
                    let var = (r0..r1).map(|s| (k * ts..(k + 1) * ts).map(|b| res[s][b] * res[s][b]).sum::<f64>()).sum::<f64>()
                        / ((r1 - r0) * ts) as f64;
                    10.0 * (1.0 / var.max(1e-4)).log10()
                })
                .collect()
        };
        Some(Demod {
            header_soft: soft[..nh].iter().flatten().copied().collect(),
            tile_bits,
            tile_snr_data_db: tile_snr(nh, rows),
            tile_snr_db: tile_snr(0, nh),
            drift_ppm,
        })
    }
}
