//! Acoustic channel simulator calibrated from measurements (`ofdm/data/channel_*.json`):
//! rx = gain * (tx * room IR), resampled by the sound-card clock offset, + measured background
//! noise + time-varying interferers.

use crate::dsp;
use num_complex::Complex64;
use rand::rngs::StdRng;
use rand::SeedableRng;
use rand_distr::{Distribution, StandardNormal};
use serde::{Deserialize, Serialize};

/// Band-limited Gaussian noise at `sir_db` relative to the received signal; `duty` < 1 gates it
/// in 0.25 s bursts.
#[derive(Clone, Debug)]
pub struct Interferer {
    pub f_lo: f64,
    pub f_hi: f64,
    pub sir_db: f64,
    pub duty: f64,
}

impl Interferer {
    pub fn new(f_lo: f64, f_hi: f64, sir_db: f64) -> Self {
        Self { f_lo, f_hi, sir_db, duty: 1.0 }
    }

    pub fn render(&self, n: usize, fs: f64, ref_rms: f64, rng: &mut StdRng) -> Vec<f64> {
        let m = dsp::next_fast_len(n);
        let mut spec = vec![Complex64::new(0.0, 0.0); m / 2 + 1];
        for (k, c) in spec.iter_mut().enumerate() {
            let f = k as f64 * fs / m as f64;
            if f >= self.f_lo && f <= self.f_hi {
                *c = Complex64::new(StandardNormal.sample(rng), StandardNormal.sample(rng));
            }
        }
        let mut x = dsp::irfft(&spec, m);
        x.truncate(n);
        let k = ref_rms * 10f64.powf(-self.sir_db / 20.0) / (dsp::rms(&x) + 1e-12);
        x.iter_mut().for_each(|v| *v *= k);
        if self.duty < 1.0 {
            let seg = (0.25 * fs) as usize;
            let mut on = true;
            for (i, v) in x.iter_mut().enumerate() {
                if i % seg == 0 {
                    on = rand::Rng::gen::<f64>(rng) < self.duty;
                }
                *v = if on { *v / self.duty.sqrt() } else { 0.0 };
            }
        }
        x
    }
}

#[derive(Serialize, Deserialize, Clone)]
pub struct ChannelData {
    pub ir: Vec<f64>,
    pub gain: f64,
    pub noise_f: Vec<f64>,
    pub noise_psd: Vec<f64>,
    pub noise_rms: f64,
}

impl ChannelData {
    pub fn path(name: &str) -> String {
        format!("{}/ofdm/data/channel_{}.json", env!("CARGO_MANIFEST_DIR"), name)
    }
    pub fn load(name: &str) -> anyhow::Result<Self> {
        Ok(serde_json::from_str(&std::fs::read_to_string(Self::path(name))?)?)
    }
}

pub struct AcousticChannel {
    pub data: ChannelData,
    pub drift_ppm: f64,
    pub rng: StdRng,
}

impl AcousticChannel {
    pub fn new(name: &str, seed: u64) -> anyhow::Result<Self> {
        Ok(Self { data: ChannelData::load(name)?, drift_ppm: 1.0, rng: StdRng::seed_from_u64(seed) })
    }

    fn noise(&mut self, n: usize, fs: f64) -> Vec<f64> {
        let m = dsp::next_fast_len(n);
        let spec: Vec<Complex64> = (0..m / 2 + 1)
            .map(|k| {
                let shape = dsp::interp(k as f64 * fs / m as f64, &self.data.noise_f, &self.data.noise_psd).sqrt();
                Complex64::new(StandardNormal.sample(&mut self.rng), StandardNormal.sample(&mut self.rng)) * shape
            })
            .collect();
        let mut x = dsp::irfft(&spec, m);
        x.truncate(n);
        let k = self.data.noise_rms / (dsp::rms(&x) + 1e-12);
        x.iter_mut().for_each(|v| *v *= k);
        x
    }

    pub fn apply(&mut self, tx: &[f64], fs: f64, interferers: &[Interferer]) -> Vec<f64> {
        let lead = (0.5 * fs) as usize;
        let mut padded = vec![0.0; lead];
        padded.extend_from_slice(tx);
        padded.extend(std::iter::repeat_n(0.0, fs as usize / 2));
        let mut y: Vec<f64> = dsp::convolve(&padded, &self.data.ir).iter().map(|v| v * self.data.gain).collect();
        if self.drift_ppm != 0.0 {
            let d = 1.0 + self.drift_ppm * 1e-6;
            let src = y.clone();
            for (t, v) in y.iter_mut().enumerate() {
                let x = t as f64 * d;
                let i = x.floor() as usize;
                *v = if i + 1 < src.len() { src[i] + (src[i + 1] - src[i]) * (x - i as f64) } else { *src.last().unwrap() };
            }
        }
        let sig_rms = dsp::rms(&y[lead..(lead + tx.len()).min(y.len())]);
        let n = y.len();
        for (v, w) in y.iter_mut().zip(self.noise(n, fs)) {
            *v += w;
        }
        for it in interferers {
            for (v, w) in y.iter_mut().zip(it.render(n, fs, sig_rms, &mut self.rng)) {
                *v += w;
            }
        }
        y
    }
}
