//! One device running the `dos` adaptive OFDM link, driven by a host that owns the audio.
//!
//! This is `net.rs`'s modem loop without the threads and sound card: the leader sends a frame and
//! listens; the follower answers every frame it hears; a station ignores what it recorded while
//! transmitting plus a reverb tail. Traffic is a file transfer (`FileSender` / `FileReceiver`),
//! so the on-air format is exactly the `dos` one.
//!
//! The host feeds microphone samples with `push_audio`, asks `poll(now)` for a waveform to play,
//! and calls `tx_done(now)` when playback has finished.

use dos::controllers::ThompsonSw;
use dos::dsp;
use dos::link::receive_frame;
use dos::node::Node;
use dos::phy::{Phy, PhyConfig};
use dos::traffic::{FileReceiver, FileSender};
use serde::Serialize;

const TAIL_S: f64 = 0.7;
const DECODE_MARGIN_S: f64 = 0.4;
const FALSE_ALARM_SNR_DB: f64 = 3.0;
const SEARCH_EVERY_S: f64 = 0.5;
/// Wait this long after the end of a heard frame before answering. The peer ignores its microphone
/// for TAIL_S after it stops transmitting (plus output/input latency); answering sooner can put
/// our sync chirp inside that window, and the whole frame is missed.
const ANSWER_GUARD_S: f64 = TAIL_S + 0.4;

/// File envelope inside the transferred byte stream: name and type travel with the data.
/// `DOSW | version | flags | name_len | name | mime_len | mime | data`. A stream without the
/// magic (e.g. from `dos duplex`) is delivered as raw bytes.
pub const ENVELOPE_MAGIC: &[u8; 4] = b"DOSW";
pub const FLAG_MESSAGE: u8 = 1;
pub const FLAG_GZIP: u8 = 2;

pub fn wrap_envelope(name: &str, mime: &str, flags: u8, data: &[u8]) -> Vec<u8> {
    let cut = |s: &str, n: usize| {
        let mut e = s.len().min(n);
        while !s.is_char_boundary(e) {
            e -= 1;
        }
        s.as_bytes()[..e].to_vec()
    };
    let (name, mime) = (cut(name, 200), cut(mime, 100));
    let mut out = ENVELOPE_MAGIC.to_vec();
    out.extend([1, flags, name.len() as u8]);
    out.extend(&name);
    out.push(mime.len() as u8);
    out.extend(&mime);
    out.extend_from_slice(data);
    out
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Delivered {
    pub name: String,
    pub mime: String,
    pub flags: u8,
    #[serde(skip)]
    pub data: Vec<u8>,
}

pub fn unwrap_envelope(buf: &[u8]) -> Delivered {
    let raw = || Delivered { name: String::new(), mime: "application/octet-stream".into(), flags: 0, data: buf.to_vec() };
    if buf.len() < 8 || &buf[..4] != ENVELOPE_MAGIC || buf[4] != 1 {
        return raw();
    }
    let flags = buf[5];
    let nl = buf[6] as usize;
    let Some(name) = buf.get(7..7 + nl) else { return raw() };
    let Some(&ml) = buf.get(7 + nl) else { return raw() };
    let Some(mime) = buf.get(8 + nl..8 + nl + ml as usize) else { return raw() };
    Delivered {
        name: String::from_utf8_lossy(name).into(),
        mime: String::from_utf8_lossy(mime).into(),
        flags,
        data: buf[8 + nl + ml as usize..].to_vec(),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Leader,
    Follower,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    /// about to send the next frame (waits out the answer guard first)
    Ready,
    Transmitting,
    Listening,
    Done,
}

/// What the last received peer frame looked like, tile by tile (for the link view).
#[derive(Clone, Debug, Default, Serialize)]
pub struct LastFrame {
    pub header_ok: bool,
    pub seq: Option<u16>,
    pub levels: Vec<u8>,
    pub tile_ok: Vec<bool>,
    pub tile_snr_db: Vec<f64>,
    pub payload_bytes: usize,
    pub snr_median_db: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct Status {
    pub role: Role,
    pub state: State,
    pub frames_sent: u64,
    pub frames_heard: u64,
    pub frames_missed: u64,
    pub false_alarms: u64,
    /// bytes still to deliver (leader) / of the incoming stream (follower)
    pub send_total: usize,
    pub send_undelivered: usize,
    pub recv_total: usize,
    pub recv_progress: f64,
    /// map this station is using for its own frames
    pub out_levels: Vec<u8>,
    pub last: Option<LastFrame>,
    pub log: Vec<String>,
}

pub struct Station {
    pub phy: Phy,
    role: Role,
    node: Node<FileSender>,
    send_total: usize,
    rx: FileReceiver,
    delivered: bool,
    rec: Vec<f64>,
    rec_start: usize,
    listen_from: usize,
    last_search_len: usize,
    state: State,
    deadline_ms: f64,
    /// absolute sample index before which this station must not start transmitting
    not_before: usize,
    heard_end: usize,
    frames_heard: u64,
    false_alarms: u64,
    last: Option<LastFrame>,
    log: Vec<String>,
    completed: Vec<Delivered>,
}

impl Station {
    /// `send`: the byte stream this station delivers (already enveloped), if any.
    /// The leader drives the exchange; a follower only answers.
    pub fn new(cfg: PhyConfig, role: Role, send: Option<Vec<u8>>, seed: u64) -> Result<Self, String> {
        validate(&cfg)?;
        let phy = Phy::new(cfg);
        let data = send.unwrap_or_default();
        let send_total = data.len();
        let ctrl = ThompsonSw::default_for(&phy.cfg, seed);
        let node = Node::with_controller(&phy, FileSender::new(data, seed), Box::new(ctrl));
        let state = if role == Role::Leader { State::Ready } else { State::Listening };
        Ok(Self {
            phy,
            role,
            node,
            send_total,
            rx: FileReceiver::default(),
            delivered: false,
            rec: Vec::new(),
            rec_start: 0,
            listen_from: 0,
            last_search_len: 0,
            state,
            deadline_ms: f64::INFINITY,
            not_before: 0,
            heard_end: 0,
            frames_heard: 0,
            false_alarms: 0,
            last: None,
            log: Vec::new(),
            completed: Vec::new(),
        })
    }

    fn rec_end(&self) -> usize {
        self.rec_start + self.rec.len()
    }

    fn say(&mut self, msg: String) {
        self.log.push(msg);
        if self.log.len() > 60 {
            self.log.remove(0);
        }
    }

    pub fn push_audio(&mut self, samples: &[f32]) {
        self.rec.extend(samples.iter().map(|&v| v as f64));
        let keep = 4 * self.phy.cfg.frame_len();
        if self.rec.len() > keep + self.phy.cfg.fs {
            let drop = self.rec.len() - keep;
            self.rec.drain(..drop);
            self.rec_start += drop;
        }
    }

    fn sending_done(&self) -> bool {
        self.send_total == 0 || self.node.traffic.done()
    }

    /// A waveform to play now, if this station wants the air.
    pub fn poll(&mut self, now_ms: f64) -> Option<Vec<f32>> {
        match self.state {
            State::Done | State::Transmitting => None,
            State::Ready => (self.rec_end() >= self.not_before).then(|| self.transmit()),
            State::Listening => {
                if let Some(seg) = self.find_frame() {
                    self.on_frame(&seg);
                    if self.role == Role::Leader && self.sending_done() {
                        self.state = State::Done;
                        self.say("everything acknowledged".into());
                        return None;
                    }
                    self.not_before = self.heard_end + (ANSWER_GUARD_S * self.phy.cfg.fs as f64) as usize;
                    self.state = State::Ready;
                    return self.poll(now_ms);
                }
                if self.role == Role::Leader && now_ms > self.deadline_ms {
                    self.node.on_lost();
                    self.say("no answer from the other device (timeout), sending again".into());
                    return Some(self.transmit());
                }
                None
            }
        }
    }

    fn transmit(&mut self) -> Vec<f32> {
        self.state = State::Transmitting;
        self.node.make_frame(&self.phy).iter().map(|&v| v as f32).collect()
    }

    pub fn tx_done(&mut self, now_ms: f64) {
        if self.state != State::Transmitting {
            return;
        }
        self.listen_from = self.rec_end() + (TAIL_S * self.phy.cfg.fs as f64) as usize;
        self.last_search_len = 0;
        self.deadline_ms = now_ms + (self.phy.cfg.frame_s() + 8.0) * 1000.0;
        self.state = State::Listening;
    }

    /// `net.rs` wait_frame: one complete peer frame after `listen_from`, or None for now.
    fn find_frame(&mut self) -> Option<Vec<f64>> {
        let cfg = &self.phy.cfg;
        let (frame, sync, fs) = (cfg.frame_len(), self.phy.sync.len(), cfg.fs);
        if self.listen_from < self.rec_start {
            self.listen_from = self.rec_start;
        }
        let mut avail = self.rec_end().saturating_sub(self.listen_from);
        if avail > 3 * frame {
            self.listen_from += avail - 2 * frame;
            avail = 2 * frame;
            self.last_search_len = 0;
        }
        let margin = (DECODE_MARGIN_S * fs as f64) as usize;
        let step = (SEARCH_EVERY_S * fs as f64) as usize;
        // a frame can only be complete once a whole frame has arrived after listen_from
        if avail < frame + margin || avail < self.last_search_len + step {
            return None;
        }
        self.last_search_len = avail;
        // only the newest frame-length (plus overlap) can hold a frame that just completed;
        // anything older was already covered by an earlier search
        let win = (frame + margin + sync + 2 * step).min(avail);
        let wstart = self.listen_from + avail - win;
        let off = wstart - self.rec_start;
        let x = &self.rec[off..off + win];
        let st = self.phy.find_sync(x)?;
        let end = st - sync + frame + margin;
        if x.len() < end {
            return None;
        }
        let begin = (st - sync).saturating_sub(fs / 10);
        let seg = x[begin..end].to_vec();
        let r = receive_frame(&self.phy, &seg, None, None);
        // NaN (no sync at all) counts as noise, as in net.rs
        let snr = dsp::median(&r.tile_snr_db);
        if r.header.is_none() && (snr.is_nan() || snr <= FALSE_ALARM_SNR_DB) {
            self.false_alarms += 1;
            self.listen_from = wstart + st;
            self.last_search_len = 0;
            return None;
        }
        self.heard_end = wstart + st - sync + frame;
        self.listen_from = wstart + end;
        self.last_search_len = 0;
        Some(seg)
    }

    fn on_frame(&mut self, seg: &[f64]) {
        self.frames_heard += 1;
        let rx = &mut self.rx;
        let r = self.node.on_receive(&self.phy, seg, |b| rx.put(b));
        let ok = r.tile_ok.iter().filter(|&&o| o).count();
        let snr = dsp::median(&r.tile_snr_db);
        let msg = match &r.header {
            Some(h) => format!("heard frame {}: {ok}/{} tiles, {} B, SNR {snr:.1} dB", h.seq, r.tile_ok.len(), r.payload_bytes),
            None => format!("heard a frame but lost its header (SNR {snr:.1} dB)"),
        };
        self.last = Some(LastFrame {
            header_ok: r.header.is_some(),
            seq: r.header.as_ref().map(|h| h.seq),
            levels: r.header.as_ref().map(|h| h.levels.clone()).unwrap_or_default(),
            tile_ok: r.tile_ok.clone(),
            tile_snr_db: r.tile_snr_db.iter().map(|v| if v.is_nan() { -10.0 } else { *v }).collect(),
            payload_bytes: r.payload_bytes,
            snr_median_db: if snr.is_nan() { -10.0 } else { snr },
        });
        self.say(msg);
        if !self.delivered && self.rx.done() {
            self.delivered = true;
            let d = unwrap_envelope(&self.rx.buf);
            self.say(format!("received {} ({} B)", if d.name.is_empty() { "a message".into() } else { d.name.clone() }, d.data.len()));
            self.completed.push(d);
        }
    }

    pub fn stop(&mut self) {
        self.state = State::Done;
    }

    pub fn take_completed(&mut self) -> Option<Delivered> {
        (!self.completed.is_empty()).then(|| self.completed.remove(0))
    }

    pub fn status(&self) -> Status {
        Status {
            role: self.role,
            state: self.state,
            frames_sent: self.node.stats.frames,
            frames_heard: self.frames_heard,
            frames_missed: self.node.stats.lost_rx,
            false_alarms: self.false_alarms,
            send_total: self.send_total,
            send_undelivered: if self.send_total == 0 { 0 } else { self.node.traffic.undelivered() },
            recv_total: self.rx.buf.len(),
            recv_progress: self.rx.progress(),
            out_levels: self.node.out_levels.clone(),
            last: self.last.clone(),
            log: self.log.clone(),
        }
    }
}

/// The header must fit one RS(255) codeword with room for its 64 info bytes.
pub fn validate(cfg: &PhyConfig) -> Result<(), String> {
    if cfg.f_lo < 50.0 || cfg.f_hi > cfg.fs as f64 / 2.0 - 200.0 || cfg.f_lo >= cfg.f_hi {
        return Err(format!("band {:.0}-{:.0} Hz is outside 50 Hz..{:.0} Hz", cfg.f_lo, cfg.f_hi, cfg.fs as f64 / 2.0 - 200.0));
    }
    if cfg.n_tiles() == 0 || cfg.header_bytes() < dos::frame::HEADER_INFO + 16 {
        let min_hz = ((dos::frame::HEADER_INFO + 16) * 8 / cfg.n_header.max(1)) as f64 * cfg.df();
        return Err(format!("{:.0} Hz is too narrow: the frame header needs at least {:.0} Hz", cfg.f_hi - cfg.f_lo, min_hz.ceil()));
    }
    let nt = cfg.n_tiles();
    let header_info = 4 + 2 * (2 * nt).div_ceil(8) + nt.div_ceil(8);
    if header_info > dos::frame::HEADER_INFO {
        let max_hz = (cfg.tile_sc as f64 * cfg.df()) * max_tiles() as f64;
        return Err(format!("{nt} tiles is too wide: the header's tile maps fit at most {} tiles ({:.0} Hz)", max_tiles(), max_hz));
    }
    Ok(())
}

/// Most tiles whose two level maps and ACK bitmap fit the 64-byte header.
pub fn max_tiles() -> usize {
    (1usize..512).take_while(|&nt| 4 + 2 * (2 * nt).div_ceil(8) + nt.div_ceil(8) <= dos::frame::HEADER_INFO).last().unwrap_or(0)
}

pub fn config(f_lo: f64, f_hi: f64) -> PhyConfig {
    PhyConfig { f_lo, f_hi, ..PhyConfig::default() }
}
