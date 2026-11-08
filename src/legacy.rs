//! The original minimodem file transfer (100 baud MFSK through the external `minimodem` tool),
//! wire-compatible with the Python version (`main.py`): Stop-and-Wait ARQ packets
//! ([type u8][seq u32][total u32][payload], big endian), framed as raw | CRC32 -> optional
//! Reed-Solomon -> COBS, sent as base64 between BEGIN_DATA / END_DATA markers.

use crate::rs;
use base64::Engine;
use std::io::{Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

const BEGIN: &[u8] = b"BEGIN_DATA\n";
const END: &[u8] = b"\nEND_DATA\n";
const LEAD_S: f64 = 1.0;

pub fn cobs_encode(data: &[u8]) -> Vec<u8> {
    if data.is_empty() {
        return vec![0];
    }
    let mut out = vec![0u8];
    let mut code_idx = 0;
    for &b in data {
        if b == 0 {
            out[code_idx] = (out.len() - code_idx) as u8;
            code_idx = out.len();
            out.push(0);
        } else {
            out.push(b);
            if out.len() - code_idx == 255 {
                out[code_idx] = 255;
                code_idx = out.len();
                out.push(0);
            }
        }
    }
    out[code_idx] = (out.len() - code_idx) as u8;
    out.push(0);
    out
}

pub fn cobs_decode(framed: &[u8]) -> Option<Vec<u8>> {
    let end = framed.iter().position(|&b| b == 0)?;
    let f = &framed[..end];
    let mut out = vec![];
    let mut i = 0;
    while i < f.len() {
        let code = f[i] as usize;
        if code == 0 || i + code > f.len() + 1 {
            return None;
        }
        out.extend_from_slice(&f[i + 1..(i + code).min(f.len())]);
        if code < 255 && i + code < f.len() {
            out.push(0);
        }
        i += code;
    }
    Some(out)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Data = 0,
    Ack = 1,
    Nak = 2,
    Eot = 3,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ArqPacket {
    pub kind: Kind,
    pub seq: u32,
    pub total: u32,
    pub payload: Vec<u8>,
}

impl ArqPacket {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut b = vec![self.kind as u8];
        b.extend_from_slice(&self.seq.to_be_bytes());
        b.extend_from_slice(&self.total.to_be_bytes());
        b.extend_from_slice(&self.payload);
        b
    }
    pub fn from_bytes(b: &[u8]) -> Option<Self> {
        if b.len() < 9 {
            return None;
        }
        let kind = match b[0] {
            0 => Kind::Data,
            1 => Kind::Ack,
            2 => Kind::Nak,
            3 => Kind::Eot,
            _ => return None,
        };
        Some(Self {
            kind,
            seq: u32::from_be_bytes(b[1..5].try_into().unwrap()),
            total: u32::from_be_bytes(b[5..9].try_into().unwrap()),
            payload: b[9..].to_vec(),
        })
    }
}

/// raw | CRC32 -> optional RS(fec) -> COBS.
pub struct Framer {
    pub fec: usize,
}

impl Framer {
    pub fn encode(&self, raw: &[u8]) -> Vec<u8> {
        let mut d = raw.to_vec();
        d.extend_from_slice(&crc32fast::hash(raw).to_be_bytes());
        if self.fec > 0 {
            d = rs::encode(&d, self.fec);
        }
        cobs_encode(&d)
    }
    pub fn decode(&self, wire: &[u8]) -> Option<Vec<u8>> {
        let mut d = cobs_decode(wire)?;
        if self.fec > 0 {
            d = rs::decode(&d, self.fec)?;
        }
        if d.len() < 4 {
            return None;
        }
        let (raw, crc) = d.split_at(d.len() - 4);
        (crc32fast::hash(raw).to_be_bytes() == crc).then(|| raw.to_vec())
    }
}

pub fn wrap(frame: &[u8]) -> Vec<u8> {
    [BEGIN, base64::engine::general_purpose::STANDARD.encode(frame).as_bytes(), END].concat()
}

pub fn airtime(frame_len: usize, baud: f64) -> f64 {
    let chars = BEGIN.len() + 4 * frame_len.div_ceil(3) + END.len();
    LEAD_S + chars as f64 * 10.0 / baud
}

/// Pop every complete BEGIN..END frame out of `buf`.
pub fn extract_frames(buf: &mut Vec<u8>) -> Vec<Vec<u8>> {
    let mut out = vec![];
    loop {
        let Some(s) = find(buf, BEGIN, 0) else {
            let keep = buf.len().saturating_sub(BEGIN.len());
            buf.drain(..keep);
            return out;
        };
        let Some(e) = find(buf, END, s + BEGIN.len()) else {
            buf.drain(..s);
            return out;
        };
        let body: Vec<u8> = buf[s + BEGIN.len()..e].iter().copied().filter(|c| !c.is_ascii_whitespace()).collect();
        buf.drain(..e + END.len());
        if let Ok(f) = base64::engine::general_purpose::STANDARD.decode(&body) {
            out.push(f);
        }
    }
}

fn find(h: &[u8], n: &[u8], from: usize) -> Option<usize> {
    (from..=h.len().saturating_sub(n.len())).find(|&i| h[i..].starts_with(n))
}

#[derive(Clone)]
pub struct ModemArgs {
    pub minimodem: String,
    pub baud: f64,
    pub mark: Option<f64>,
    pub space: Option<f64>,
    pub volume: Option<f64>,
    pub turnaround: f64,
}

impl ModemArgs {
    fn common(&self) -> Vec<String> {
        let mut a = vec!["--quiet".to_string()];
        if let Some(m) = self.mark {
            a.extend(["--mark".into(), m.to_string()]);
        }
        if let Some(s) = self.space {
            a.extend(["--space".into(), s.to_string()]);
        }
        a.push(self.baud.to_string());
        a
    }
}

/// Half-duplex bridge: a minimodem listener runs except while a frame is being played.
fn bridge(args: ModemArgs, tx: Receiver<Vec<u8>>, rx: Sender<Vec<u8>>) {
    let start_rx = |a: &ModemArgs| -> Child {
        Command::new(&a.minimodem).arg("--rx").args(a.common()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().expect("minimodem --rx")
    };
    let (chunk_tx, chunk_rx) = channel::<Vec<u8>>();
    let spawn_reader = |child: &mut Child, out: Sender<Vec<u8>>| {
        let mut so = child.stdout.take().unwrap();
        std::thread::spawn(move || {
            let mut b = [0u8; 4096];
            while let Ok(n) = so.read(&mut b) {
                if n == 0 || out.send(b[..n].to_vec()).is_err() {
                    return;
                }
            }
        });
    };
    let mut child = start_rx(&args);
    spawn_reader(&mut child, chunk_tx.clone());
    let mut buf = vec![];
    loop {
        match tx.recv_timeout(Duration::from_millis(50)) {
            Ok(frame) => {
                let t = Instant::now();
                while t.elapsed() < Duration::from_secs_f64(args.turnaround) {
                    while let Ok(c) = chunk_rx.try_recv() {
                        buf.extend(c);
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
                let _ = child.kill();
                let _ = child.wait();
                let mut cmd = Command::new(&args.minimodem);
                cmd.arg("--tx");
                if let Some(v) = args.volume {
                    cmd.args(["--volume", &v.to_string()]);
                }
                let mut p = cmd.args(args.common()).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().expect("minimodem --tx");
                eprintln!("TX {} bytes (~{:.1}s on air)", frame.len(), airtime(frame.len(), args.baud));
                let _ = p.stdin.take().unwrap().write_all(&wrap(&frame));
                let _ = p.wait();
                buf.clear();
                child = start_rx(&args);
                spawn_reader(&mut child, chunk_tx.clone());
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                let _ = child.kill();
                return;
            }
        }
        while let Ok(c) = chunk_rx.try_recv() {
            buf.extend(c);
        }
        for f in extract_frames(&mut buf) {
            eprintln!("RX frame {} bytes", f.len());
            if rx.send(f).is_err() {
                return;
            }
        }
        if buf.len() > 16384 {
            buf.clear();
        }
    }
}

pub struct TransferArgs {
    pub modem: ModemArgs,
    pub payload_size: usize,
    pub fec: usize,
    pub max_retries: u32,
    pub timeout: Option<f64>,
}

impl TransferArgs {
    fn ack_timeout(&self) -> f64 {
        let flen = |payload: usize| {
            let raw = 9 + payload + 4 + self.fec;
            raw + raw / 254 + 2
        };
        self.timeout.unwrap_or(airtime(flen(self.payload_size), self.modem.baud) + airtime(flen(0), self.modem.baud) + 2.0 * self.modem.turnaround + 3.0)
    }
}

fn start(a: &TransferArgs) -> (Sender<Vec<u8>>, Receiver<Vec<u8>>, Framer) {
    let (tx_s, tx_r) = channel();
    let (rx_s, rx_r) = channel();
    let m = a.modem.clone();
    std::thread::spawn(move || bridge(m, tx_r, rx_s));
    (tx_s, rx_r, Framer { fec: a.fec })
}

/// Send `data` reliably (stop-and-wait, EOT acknowledged). Returns true if everything was ACKed.
pub fn send(a: &TransferArgs, data: &[u8]) -> bool {
    let (tx, rx, framer) = start(a);
    let timeout = a.ack_timeout();
    let chunks: Vec<&[u8]> = data.chunks(a.payload_size.max(1)).collect();
    let total = chunks.len() as u32;
    let mut pkts: Vec<ArqPacket> = chunks.iter().enumerate().map(|(i, c)| ArqPacket { kind: Kind::Data, seq: i as u32, total, payload: c.to_vec() }).collect();
    pkts.push(ArqPacket { kind: Kind::Eot, seq: total, total, payload: vec![] });
    eprintln!("Sending {total} packets (+EOT), ack timeout {timeout:.1}s");
    for p in &pkts {
        let mut acked = false;
        for attempt in 0..=a.max_retries {
            eprintln!("Sending {:?} {}/{}{}", p.kind, p.seq, p.total, if attempt > 0 { format!(" (retry {attempt})") } else { String::new() });
            let _ = tx.send(framer.encode(&p.to_bytes()));
            let t = Instant::now();
            while let Some(left) = Duration::from_secs_f64(timeout).checked_sub(t.elapsed()) {
                match rx.recv_timeout(left) {
                    Ok(w) => {
                        if let Some(r) = framer.decode(&w).and_then(|raw| ArqPacket::from_bytes(&raw)) {
                            if r.kind == Kind::Ack && r.seq == p.seq {
                                acked = true;
                                break;
                            }
                        }
                    }
                    Err(_) => break,
                }
            }
            if acked {
                eprintln!("ACK {}/{}", p.seq, p.total);
                break;
            }
        }
        if !acked {
            eprintln!("Maximum retries exceeded for packet {}", p.seq);
            return false;
        }
    }
    true
}

/// Receive until EOT (then linger to re-ACK a lost final ACK). Returns the data and completeness.
pub fn receive(a: &TransferArgs, max_wait: Duration) -> (Vec<u8>, bool) {
    let (tx, rx, framer) = start(a);
    let (mut data, mut next, mut finished) = (vec![], 0u32, None::<Instant>);
    let linger = Duration::from_secs_f64(a.ack_timeout() + a.modem.turnaround);
    let t0 = Instant::now();
    loop {
        if finished.is_some_and(|f| f.elapsed() > linger) || t0.elapsed() > max_wait {
            break;
        }
        let Ok(w) = rx.recv_timeout(Duration::from_millis(200)) else { continue };
        let Some(p) = framer.decode(&w).and_then(|raw| ArqPacket::from_bytes(&raw)) else {
            eprintln!("bad frame dropped");
            continue;
        };
        match p.kind {
            Kind::Data if p.seq == next => {
                data.extend_from_slice(&p.payload);
                next += 1;
                eprintln!("Received DATA {}/{}", p.seq + 1, p.total);
            }
            Kind::Eot if p.seq == next => {
                eprintln!("Received EOT");
                finished.get_or_insert_with(Instant::now);
            }
            Kind::Data | Kind::Eot => eprintln!("Duplicate {:?} {}, re-ACKing", p.kind, p.seq),
            _ => continue,
        }
        if p.seq <= next {
            let ack = ArqPacket { kind: Kind::Ack, seq: p.seq, total: p.total, payload: vec![] };
            let _ = tx.send(framer.encode(&ack.to_bytes()));
        }
    }
    (data, finished.is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cobs_roundtrip() {
        for n in [0usize, 1, 2, 253, 254, 255, 600] {
            let d: Vec<u8> = (0..n).map(|i| if i % 7 == 0 { 0 } else { (i * 31 % 251) as u8 }).collect();
            assert_eq!(cobs_decode(&cobs_encode(&d)).unwrap(), d, "n={n}");
            let d: Vec<u8> = (0..n).map(|i| (i % 255 + 1) as u8).collect();
            assert_eq!(cobs_decode(&cobs_encode(&d)).unwrap(), d, "nonzero n={n}");
        }
    }

    #[test]
    fn framer_and_markers() {
        let f = Framer { fec: 16 };
        let p = ArqPacket { kind: Kind::Data, seq: 3, total: 9, payload: b"hello\0world".to_vec() };
        let w = f.encode(&p.to_bytes());
        let mut buf = [b"\xff\x13noise".to_vec(), wrap(&w), b"tail".to_vec()].concat();
        let frames = extract_frames(&mut buf);
        assert_eq!(ArqPacket::from_bytes(&f.decode(&frames[0]).unwrap()).unwrap(), p);
    }
}
