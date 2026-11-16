//! Simulated air between two stations, streamed in real time.
//!
//! Each direction uses `dos::channel::AcousticChannel` built from the measured room
//! (`data/channel_*.json`: impulse response, gain, sound-card drift, background noise), plus
//! optional interferers from `dos::scenario`. A transmitted frame is rendered through the channel
//! once and then handed out block by block; between frames a station hears the measured noise.

use dos::channel::{AcousticChannel, ChannelData, Interferer};
use std::collections::VecDeque;

const NOISE_LOOP_S: f64 = 4.0;
const SELF_GAIN: f32 = 0.25;

pub struct Air {
    fs: usize,
    channels: [AcousticChannel; 2],
    noise: [Vec<f32>; 2],
    noise_pos: [usize; 2],
    incoming: [VecDeque<f32>; 2],
    pub interferers: Vec<Interferer>,
}

impl Air {
    /// `a_to_b`: what station 1 hears from station 0; `b_to_a` the reverse.
    pub fn new(a_to_b: ChannelData, b_to_a: ChannelData, fs: usize, seed: u64) -> Self {
        let mut channels = [AcousticChannel::from_data(a_to_b, seed), AcousticChannel::from_data(b_to_a, seed + 1)];
        let silence = vec![0.0; (NOISE_LOOP_S * fs as f64) as usize];
        // what each station hears with nobody transmitting: channel noise only
        let noise = [
            channels[1].apply(&silence, fs as f64, &[]).iter().map(|&v| v as f32).collect(),
            channels[0].apply(&silence, fs as f64, &[]).iter().map(|&v| v as f32).collect(),
        ];
        Self { fs, channels, noise, noise_pos: [0, 0], incoming: [VecDeque::new(), VecDeque::new()], interferers: vec![] }
    }

    /// Station `from` starts playing `wave` now.
    pub fn transmit(&mut self, from: usize, wave: &[f32]) {
        let to = 1 - from;
        let tx: Vec<f64> = wave.iter().map(|&v| v as f64).collect();
        let rx = self.channels[from].apply(&tx, self.fs as f64, &self.interferers);
        mix_into(&mut self.incoming[to], rx.iter().map(|&v| v as f32));
        mix_into(&mut self.incoming[from], wave.iter().map(|&v| v * SELF_GAIN));
    }

    /// The next `n` samples station `who` hears.
    pub fn hear(&mut self, who: usize, n: usize) -> Vec<f32> {
        let mut out = Vec::with_capacity(n);
        for _ in 0..n {
            let noise = &self.noise[who];
            let bg = noise[self.noise_pos[who]];
            self.noise_pos[who] = (self.noise_pos[who] + 1) % noise.len();
            out.push(self.incoming[who].pop_front().unwrap_or(bg));
        }
        out
    }
}

fn mix_into(q: &mut VecDeque<f32>, src: impl Iterator<Item = f32>) {
    for (i, v) in src.enumerate() {
        match q.get_mut(i) {
            Some(s) => *s += v,
            None => q.push_back(v),
        }
    }
}
