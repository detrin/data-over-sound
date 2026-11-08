//! An adaptive link endpoint: header feedback, Thompson-sampling adaptation, fallback.
//!
//! The receiver of a direction owns its adaptation: it measures every tile (header SNR + block
//! outcomes), runs the controller and sends back in its own header: fb_seq (the peer frame it
//! reports on), fb_ok (per-tile ACK bitmap) and fb_levels (the map the peer should use next).
//! Without feedback for FALLBACK_AFTER frames a sender falls back to the robust all-L1 map.

use crate::controllers::{Controller, ThompsonSw};
use crate::frame::{payload_capacity, Header, NO_FEEDBACK};
use crate::link::{build_frame, receive_frame, RxResult};
use crate::phy::Phy;
use crate::traffic::Traffic;

pub const FALLBACK_AFTER: u32 = 2;

#[derive(Default, Debug, Clone)]
pub struct NodeStats {
    pub frames: u64,
    pub lost_rx: u64,
    pub payload_bytes: u64,
}

pub struct Node<T: Traffic> {
    pub traffic: T,
    pub ctrl: Box<dyn Controller>,
    pub out_levels: Vec<u8>,
    told_levels: Vec<u8>,
    fb: (u16, Vec<bool>, Vec<u8>),
    seq: u16,
    missed: u32,
    pub stats: NodeStats,
}

impl<T: Traffic> Node<T> {
    pub fn new(phy: &Phy, traffic: T, seed: u64) -> Self {
        Self::with_controller(phy, traffic, Box::new(ThompsonSw::default_for(&phy.cfg, seed)))
    }

    pub fn with_controller(phy: &Phy, traffic: T, ctrl: Box<dyn Controller>) -> Self {
        let n = phy.cfg.n_tiles();
        Self {
            traffic,
            ctrl,
            out_levels: vec![1; n],
            told_levels: vec![1; n],
            fb: (NO_FEEDBACK, vec![false; n], vec![1; n]),
            seq: 0,
            missed: 0,
            stats: NodeStats::default(),
        }
    }

    pub fn seq(&self) -> u16 {
        self.seq
    }

    pub fn make_frame(&mut self, phy: &Phy) -> Vec<f64> {
        let caps: Vec<usize> = self.out_levels.iter().map(|&l| payload_capacity(&phy.cfg, l)).collect();
        let mut blocks = self.traffic.fill(self.seq, &caps).into_iter();
        let h = Header {
            seq: self.seq,
            levels: self.out_levels.clone(),
            fb_seq: self.fb.0,
            fb_ok: self.fb.1.clone(),
            fb_levels: self.fb.2.clone(),
        };
        let (wave, _) = build_frame(phy, &h, &self.out_levels, |_| blocks.next().unwrap_or_default());
        self.seq = (self.seq + 1) % NO_FEEDBACK;
        self.stats.frames += 1;
        wave
    }

    /// Decode a recording of the peer's frame; `deliver` gets every decoded tile payload.
    pub fn on_receive(&mut self, phy: &Phy, rec: &[f64], mut deliver: impl FnMut(&[u8])) -> RxResult {
        let n = phy.cfg.n_tiles();
        let r = receive_frame(phy, rec, None, None);
        let Some(h) = r.header.clone() else {
            self.ctrl.observe(&self.told_levels, &vec![false; n], &r.tile_snr_db);
            self.missed_feedback(true);
            return r;
        };
        for b in r.blocks.iter().flatten() {
            deliver(b);
            self.stats.payload_bytes += b.len() as u64;
        }
        self.ctrl.observe(&h.levels, &r.tile_ok, &r.tile_snr_db);
        if h.fb_seq != NO_FEEDBACK {
            self.traffic.ack(h.fb_seq, &h.fb_ok);
            self.traffic.requeue_before(h.fb_seq);
            self.out_levels = h.fb_levels.clone();
            self.missed = 0;
        } else {
            self.missed_feedback(false);
        }
        self.told_levels = self.ctrl.choose();
        self.fb = (h.seq, r.tile_ok.clone(), self.told_levels.clone());
        r
    }

    /// No peer frame was heard at all (timeout).
    pub fn on_lost(&mut self) {
        self.missed_feedback(true);
    }

    fn missed_feedback(&mut self, lost: bool) {
        self.stats.lost_rx += lost as u64;
        self.missed += 1;
        if self.missed >= FALLBACK_AFTER {
            self.out_levels = vec![1; self.out_levels.len()];
            self.traffic.requeue_before(self.seq);
        }
    }
}
