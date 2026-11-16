//! The original `dos modem` transfer, without the external minimodem binary.
//!
//! On air it is what `minimodem --tx --mark 2300 --space 1900 100` produces: continuous-phase FSK,
//! asynchronous 8N1 characters (start bit = space, 8 data bits LSB first with 1 = mark, stop bit =
//! mark) carrying `legacy::wrap` text (BEGIN_DATA / base64 / END_DATA). Packets and their framing
//! are `dos::legacy` (Stop-and-Wait ARQ, CRC32, Reed-Solomon, COBS), so this talks to
//! `dos modem send/recv` on a laptop.

use dos::legacy::{airtime, extract_frames, wrap, ArqPacket, Framer, Kind};
use num_complex::Complex32;
use serde::Serialize;
use std::collections::VecDeque;
use std::f32::consts::PI;

#[derive(Clone, Copy, Debug)]
pub struct AfskConfig {
    pub fs: usize,
    pub baud: f32,
    pub mark: f32,
    pub space: f32,
    pub amplitude: f32,
    pub lead_s: f32,
    pub trail_s: f32,
}

impl Default for AfskConfig {
    /// `dos modem` defaults: 100 baud on 2300/1900 Hz, where the laptop speaker-to-mic path is flat.
    fn default() -> Self {
        Self { fs: 48_000, baud: 100.0, mark: 2300.0, space: 1900.0, amplitude: 0.5, lead_s: 0.3, trail_s: 0.1 }
    }
}

impl AfskConfig {
    pub fn samples_per_bit(&self) -> f32 {
        self.fs as f32 / self.baud
    }
}

/// Text bytes -> FSK waveform with a mark carrier before and after.
pub fn modulate(cfg: &AfskConfig, text: &[u8]) -> Vec<f32> {
    let spb = cfg.samples_per_bit();
    let mut bits: Vec<bool> = Vec::with_capacity(text.len() * 10);
    for &b in text {
        bits.push(false);
        bits.extend((0..8).map(|i| (b >> i) & 1 == 1));
        bits.push(true);
    }
    let lead = (cfg.lead_s * cfg.fs as f32) as usize;
    let trail = (cfg.trail_s * cfg.fs as f32) as usize;
    let body = (bits.len() as f32 * spb).round() as usize;
    let n = lead + body + trail;
    let mut out = Vec::with_capacity(n);
    let mut phase = 0.0f32;
    for i in 0..n {
        let in_body = i >= lead && i < lead + body;
        let f = if in_body && !bits[((i - lead) as f32 / spb) as usize] { cfg.space } else { cfg.mark };
        out.push(cfg.amplitude * phase.sin());
        phase = (phase + 2.0 * PI * f / cfg.fs as f32) % (2.0 * PI);
    }
    let ramp = (0.005 * cfg.fs as f32) as usize;
    for i in 0..ramp.min(n / 2) {
        let g = 0.5 - 0.5 * (PI * i as f32 / ramp as f32).cos();
        out[i] *= g;
        out[n - 1 - i] *= g;
    }
    out
}

/// Streaming UART over FSK. A sliding-window discriminator compares mark and space energy over the
/// last `win` samples; the window covers the later part of each bit, away from the previous bit's
/// room echo.
pub struct UartRx {
    spb: f32,
    win: usize,
    step: [Complex32; 2],
    osc: [Complex32; 2],
    hist: VecDeque<[Complex32; 2]>,
    acc: [Complex32; 2],
    since_renorm: usize,
    t: usize,
    floor: f32,
    prev_disc: f32,
    state: Rx,
}

enum Rx {
    Idle,
    /// edge = sample index where the start bit began
    Char { edge: f32, bits: u16, n: u8 },
}

impl UartRx {
    pub fn new(cfg: &AfskConfig) -> Self {
        let spb = cfg.samples_per_bit();
        let w = |f: f32| Complex32::from_polar(1.0, -2.0 * PI * f / cfg.fs as f32);
        Self {
            spb,
            win: (spb * 0.6) as usize,
            step: [w(cfg.mark), w(cfg.space)],
            osc: [Complex32::new(1.0, 0.0); 2],
            hist: VecDeque::new(),
            acc: [Complex32::new(0.0, 0.0); 2],
            since_renorm: 0,
            t: 0,
            floor: f32::INFINITY,
            prev_disc: 0.0,
            state: Rx::Idle,
        }
    }

    /// Decoded characters from the next block of audio.
    pub fn push(&mut self, samples: &[f32]) -> Vec<u8> {
        let mut out = Vec::new();
        for &x in samples {
            let v = [self.osc[0] * x, self.osc[1] * x];
            for ((osc, step), (acc, v)) in self.osc.iter_mut().zip(&self.step).zip(self.acc.iter_mut().zip(v)) {
                *osc *= step;
                *acc += v;
            }
            self.hist.push_back(v);
            if self.hist.len() > self.win {
                let old = self.hist.pop_front().unwrap();
                self.acc[0] -= old[0];
                self.acc[1] -= old[1];
            }
            self.since_renorm += 1;
            if self.since_renorm >= 48_000 {
                self.since_renorm = 0;
                for k in 0..2 {
                    self.osc[k] /= self.osc[k].norm();
                }
                self.acc = [self.hist.iter().map(|h| h[0]).sum(), self.hist.iter().map(|h| h[1]).sum()];
            }
            let (pm, ps) = (self.acc[0].norm_sqr(), self.acc[1].norm_sqr());
            let energy = pm + ps;
            self.floor = if energy < self.floor { energy } else { self.floor * 1.00002 + 1e-12 };
            let disc = (pm - ps) / (energy + 1e-12);
            let loud = energy > 6.0 * self.floor;
            let t = self.t as f32;
            match &mut self.state {
                Rx::Idle => {
                    if loud && self.prev_disc >= 0.0 && disc < 0.0 {
                        // the window is half into the start bit
                        self.state = Rx::Char { edge: t - self.win as f32 / 2.0, bits: 0, n: 0 };
                    }
                }
                Rx::Char { edge, bits, n } => {
                    // sample k (0 = start, 1..8 data, 9 = stop) when the window ends at that bit's end
                    let at = *edge + (*n as f32 + 1.0) * self.spb;
                    if t >= at {
                        let mark = disc > 0.0;
                        match *n {
                            0 if mark || !loud => self.state = Rx::Idle,
                            0 => *n = 1,
                            1..=8 => {
                                *bits |= (mark as u16) << (*n - 1);
                                *n += 1;
                            }
                            _ => {
                                if mark {
                                    out.push(*bits as u8);
                                }
                                self.state = Rx::Idle;
                            }
                        }
                    }
                }
            }
            self.prev_disc = disc;
            self.t += 1;
        }
        out
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ClassicState {
    Ready,
    Transmitting,
    Listening,
    Done,
    Failed,
}

#[derive(Clone, Debug, Serialize)]
pub struct ClassicStatus {
    pub sending: bool,
    pub state: ClassicState,
    pub packet: u32,
    pub total: u32,
    pub retries: u32,
    pub received_bytes: usize,
    pub frames_heard: u32,
    pub log: Vec<String>,
}

/// One side of the `dos modem` Stop-and-Wait transfer, driven by a host that owns the audio.
pub struct Classic {
    cfg: AfskConfig,
    framer: Framer,
    uart: UartRx,
    text: Vec<u8>,
    payload_size: usize,
    max_retries: u32,
    turnaround_ms: f64,
    // sender
    packets: Vec<ArqPacket>,
    idx: usize,
    attempt: u32,
    deadline: f64,
    // receiver
    data: Vec<u8>,
    next: u32,
    finished_at: Option<f64>,
    acks: VecDeque<(f64, Vec<f32>)>,
    delivered: Option<Vec<u8>>,
    // both
    sending: bool,
    state: ClassicState,
    mute_until: f64,
    now: f64,
    frames_heard: u32,
    retries: u32,
    log: Vec<String>,
}

const TAIL_MS: f64 = 300.0;

impl Classic {
    fn base(cfg: AfskConfig, sending: bool) -> Self {
        Self {
            uart: UartRx::new(&cfg),
            cfg,
            framer: Framer { fec: 16 },
            text: Vec::new(),
            payload_size: 64,
            max_retries: 5,
            turnaround_ms: 1000.0,
            packets: Vec::new(),
            idx: 0,
            attempt: 0,
            deadline: f64::INFINITY,
            data: Vec::new(),
            next: 0,
            finished_at: None,
            acks: VecDeque::new(),
            delivered: None,
            sending,
            state: if sending { ClassicState::Ready } else { ClassicState::Listening },
            mute_until: 0.0,
            now: 0.0,
            frames_heard: 0,
            retries: 0,
            log: Vec::new(),
        }
    }

    pub fn receiver(cfg: AfskConfig) -> Self {
        Self::base(cfg, false)
    }

    /// Sender of `data` as `dos modem send` does it: 64-byte packets, then EOT.
    pub fn sender(cfg: AfskConfig, data: &[u8]) -> Self {
        let mut s = Self::base(cfg, true);
        let chunks: Vec<&[u8]> = data.chunks(s.payload_size).collect();
        let total = chunks.len() as u32;
        s.packets = chunks.iter().enumerate().map(|(i, c)| ArqPacket { kind: Kind::Data, seq: i as u32, total, payload: c.to_vec() }).collect();
        s.packets.push(ArqPacket { kind: Kind::Eot, seq: total, total, payload: vec![] });
        s
    }

    fn say(&mut self, m: String) {
        self.log.push(m);
        if self.log.len() > 60 {
            self.log.remove(0);
        }
    }

    fn wave(&self, p: &ArqPacket) -> Vec<f32> {
        modulate(&self.cfg, &wrap(&self.framer.encode(&p.to_bytes())))
    }

    /// `legacy::TransferArgs::ack_timeout`: data frame + ACK airtime + two turnarounds + 3 s.
    fn ack_timeout_ms(&self) -> f64 {
        let flen = |payload: usize| {
            let raw = 9 + payload + 4 + self.framer.fec;
            raw + raw / 254 + 2
        };
        let b = self.cfg.baud as f64;
        (airtime(flen(self.payload_size), b) + airtime(flen(0), b) + 2.0 * self.turnaround_ms / 1000.0 + 3.0) * 1000.0
    }

    pub fn push_audio(&mut self, samples: &[f32], now_ms: f64) {
        self.now = now_ms;
        if self.state == ClassicState::Transmitting || now_ms < self.mute_until {
            self.uart.push(&vec![0.0; samples.len()]);
            return;
        }
        let chars = self.uart.push(samples);
        self.text.extend(chars);
        for f in extract_frames(&mut self.text) {
            let Some(p) = self.framer.decode(&f).and_then(|raw| ArqPacket::from_bytes(&raw)) else {
                self.say("heard a frame that failed its check, dropped".into());
                continue;
            };
            self.frames_heard += 1;
            if self.sending {
                self.on_ack(p);
            } else {
                self.on_data(p, now_ms);
            }
        }
        if self.text.len() > 16_384 {
            self.text.clear();
        }
    }

    fn on_ack(&mut self, p: ArqPacket) {
        let Some(cur) = self.packets.get(self.idx) else { return };
        if p.kind == Kind::Ack && p.seq == cur.seq && self.state == ClassicState::Listening {
            self.say(format!("ACK {}/{}", p.seq, p.total));
            self.idx += 1;
            self.attempt = 0;
            self.state = if self.idx >= self.packets.len() { ClassicState::Done } else { ClassicState::Ready };
        }
    }

    fn on_data(&mut self, p: ArqPacket, now: f64) {
        match p.kind {
            Kind::Data if p.seq == self.next => {
                self.data.extend_from_slice(&p.payload);
                self.next += 1;
                self.say(format!("received packet {}/{}", p.seq + 1, p.total));
            }
            Kind::Eot if p.seq == self.next => {
                self.say("received end of transmission".into());
                if self.finished_at.is_none() {
                    self.finished_at = Some(now);
                    self.delivered = Some(self.data.clone());
                }
            }
            Kind::Data | Kind::Eot => self.say(format!("duplicate packet {}, confirming again", p.seq)),
            _ => return,
        }
        if p.seq <= self.next {
            let ack = ArqPacket { kind: Kind::Ack, seq: p.seq, total: p.total, payload: vec![] };
            let w = self.wave(&ack);
            self.acks.push_back((now + self.turnaround_ms, w));
        }
    }

    pub fn poll(&mut self, now_ms: f64) -> Option<Vec<f32>> {
        self.now = now_ms;
        match self.state {
            ClassicState::Transmitting | ClassicState::Done | ClassicState::Failed => None,
            ClassicState::Ready if self.sending => {
                let p = self.packets[self.idx].clone();
                self.say(format!(
                    "sending {} {}/{}{}",
                    if p.kind == Kind::Eot { "EOT" } else { "packet" },
                    p.seq,
                    p.total,
                    if self.attempt > 0 { format!(" (retry {})", self.attempt) } else { String::new() }
                ));
                self.state = ClassicState::Transmitting;
                Some(self.wave(&p))
            }
            ClassicState::Listening if self.sending && now_ms > self.deadline => {
                self.attempt += 1;
                self.retries += 1;
                if self.attempt > self.max_retries {
                    self.say(format!("no ACK for packet {} after {} retries", self.idx, self.max_retries));
                    self.state = ClassicState::Failed;
                    return None;
                }
                self.state = ClassicState::Ready;
                self.poll(now_ms)
            }
            _ if !self.sending => {
                let (due, _) = self.acks.front()?;
                if *due > now_ms {
                    return None;
                }
                let (_, w) = self.acks.pop_front().unwrap();
                self.acks.clear();
                self.state = ClassicState::Transmitting;
                Some(w)
            }
            _ => None,
        }
    }

    pub fn tx_done(&mut self, now_ms: f64) {
        if self.state != ClassicState::Transmitting {
            return;
        }
        self.mute_until = now_ms + TAIL_MS;
        self.deadline = now_ms + self.ack_timeout_ms();
        self.state = ClassicState::Listening;
    }

    pub fn take_completed(&mut self) -> Option<Vec<u8>> {
        self.delivered.take()
    }

    pub fn stop(&mut self) {
        self.state = ClassicState::Done;
    }

    pub fn status(&self) -> ClassicStatus {
        ClassicStatus {
            sending: self.sending,
            state: self.state,
            packet: if self.sending { self.idx as u32 } else { self.next },
            total: if self.sending { self.packets.len() as u32 } else { self.next },
            retries: self.retries,
            received_bytes: self.data.len(),
            frames_heard: self.frames_heard,
            log: self.log.clone(),
        }
    }
}
