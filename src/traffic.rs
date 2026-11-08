//! What a node sends and receives inside its tiles.
//!
//! `Traffic` is the sending side used by `Node`: fill tiles, then learn per-tile outcomes.
//! - `FileSender` / `FileReceiver`: one byte stream with selective repeat until delivered.
//!   Tile payload: [offset 3 B][length 2 B][total 3 B][data][zero padding].
//! - `PacketQueue` / `PacketReassembly`: IP packets cut into fragments that pack the current map;
//!   failed fragments are resent up to MAX_TRIES times (beyond that TCP recovers). Fragment:
//!   [packet id 2 B][offset 2 B][length 2 B][packet length 2 B][data]; id 0 ends a block.

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::{Hash, Hasher};
use std::time::{Duration, Instant};

pub trait Traffic: Send {
    /// Payload for each tile (cap = payload bytes, 0 for pilots) of frame `seq`.
    fn fill(&mut self, seq: u16, caps: &[usize]) -> Vec<Vec<u8>>;
    /// Per-tile outcome of frame `seq` (from the peer's feedback).
    fn ack(&mut self, seq: u16, ok: &[bool]);
    /// Frames older than `seq` that never got feedback count as failed.
    fn requeue_before(&mut self, seq: u16);
}

/// Tiles ordered by capacity (largest, i.e. most reliable, first; random among equals): the oldest
/// needed data goes there, so a bad tile cannot starve a range by failing it again and again.
fn tile_order(caps: &[usize], rng: &mut StdRng) -> Vec<usize> {
    let mut keyed: Vec<(usize, f64, usize)> = caps.iter().enumerate().map(|(t, &c)| (c, rng.gen::<f64>(), t)).collect();
    keyed.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.partial_cmp(&b.1).unwrap()));
    keyed.into_iter().map(|k| k.2).collect()
}

const FILE_HDR: usize = 8;

pub struct FileSender {
    data: Vec<u8>,
    needed: Vec<bool>,
    delivered: Vec<bool>,
    inflight: HashMap<u16, Vec<(usize, usize, usize)>>,
    rng: StdRng,
}

impl FileSender {
    pub fn new(data: Vec<u8>, seed: u64) -> Self {
        let n = data.len();
        Self { data, needed: vec![true; n], delivered: vec![false; n], inflight: HashMap::new(), rng: StdRng::seed_from_u64(seed) }
    }
    pub fn done(&self) -> bool {
        self.delivered.iter().all(|&d| d)
    }
    pub fn undelivered(&self) -> usize {
        self.delivered.iter().filter(|&&d| !d).count()
    }
}

impl Traffic for FileSender {
    fn fill(&mut self, seq: u16, caps: &[usize]) -> Vec<Vec<u8>> {
        let mut out = vec![vec![]; caps.len()];
        let mut sent = vec![];
        for tile in tile_order(caps, &mut self.rng) {
            let room = caps[tile].saturating_sub(FILE_HDR);
            let Some(off) = self.needed.iter().position(|&n| n) else { break };
            if room == 0 {
                continue;
            }
            let mut end = off + 1;
            while end < self.data.len() && end - off < room && self.needed[end] {
                end += 1;
            }
            self.needed[off..end].iter_mut().for_each(|n| *n = false);
            sent.push((tile, off, end - off));
            let mut b = Vec::with_capacity(FILE_HDR + end - off);
            b.extend_from_slice(&(off as u32).to_be_bytes()[1..]);
            b.extend_from_slice(&((end - off) as u16).to_be_bytes());
            b.extend_from_slice(&(self.data.len() as u32).to_be_bytes()[1..]);
            b.extend_from_slice(&self.data[off..end]);
            out[tile] = b;
        }
        self.inflight.insert(seq, sent);
        out
    }

    fn ack(&mut self, seq: u16, ok: &[bool]) {
        for (tile, off, n) in self.inflight.remove(&seq).unwrap_or_default() {
            if ok.get(tile).copied().unwrap_or(false) {
                self.delivered[off..off + n].iter_mut().for_each(|d| *d = true);
            } else {
                for i in off..off + n {
                    self.needed[i] = !self.delivered[i];
                }
            }
        }
    }

    fn requeue_before(&mut self, seq: u16) {
        let old: Vec<u16> = self.inflight.keys().copied().filter(|&s| s < seq).collect();
        for s in old {
            self.ack(s, &[]);
        }
    }
}

#[derive(Default)]
pub struct FileReceiver {
    pub buf: Vec<u8>,
    pub have: Vec<bool>,
}

impl FileReceiver {
    pub fn put(&mut self, block: &[u8]) {
        if block.len() < FILE_HDR {
            return;
        }
        let off = u32::from_be_bytes([0, block[0], block[1], block[2]]) as usize;
        let n = u16::from_be_bytes([block[3], block[4]]) as usize;
        let total = u32::from_be_bytes([0, block[5], block[6], block[7]]) as usize;
        if self.have.is_empty() {
            self.buf = vec![0; total];
            self.have = vec![false; total];
        }
        let data = &block[FILE_HDR..(FILE_HDR + n).min(block.len())];
        let n = data.len().min(self.buf.len().saturating_sub(off));
        self.buf[off..off + n].copy_from_slice(&data[..n]);
        self.have[off..off + n].iter_mut().for_each(|h| *h = true);
    }
    pub fn done(&self) -> bool {
        !self.have.is_empty() && self.have.iter().all(|&h| h)
    }
    pub fn progress(&self) -> f64 {
        if self.have.is_empty() { 0.0 } else { self.have.iter().filter(|&&h| h).count() as f64 / self.have.len() as f64 }
    }
}

pub const FRAG_HDR: usize = 8;
pub const MAX_TRIES: u8 = 6;
pub const MAX_QUEUE_BYTES: usize = 64 * 1024;

#[derive(Default, Debug, Clone)]
pub struct QueueStats {
    pub queued: u64,
    pub dropped_full: u64,
    pub dropped_dup: u64,
    pub given_up: u64,
}

pub struct PacketQueue {
    packets: HashMap<u16, Vec<u8>>,
    ranges: VecDeque<(u16, usize, usize, u8)>,
    inflight: HashMap<u16, Vec<(usize, u16, usize, usize, u8)>>,
    waiting: HashMap<u64, u16>,
    next_pid: u16,
    rng: StdRng,
    pub stats: QueueStats,
}

fn digest(b: &[u8]) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    b.hash(&mut h);
    h.finish()
}

impl PacketQueue {
    pub fn new(seed: u64) -> Self {
        Self {
            packets: HashMap::new(),
            ranges: VecDeque::new(),
            inflight: HashMap::new(),
            waiting: HashMap::new(),
            next_pid: 1,
            rng: StdRng::seed_from_u64(seed),
            stats: QueueStats::default(),
        }
    }

    pub fn push(&mut self, pkt: Vec<u8>) {
        let h = digest(&pkt);
        if self.waiting.contains_key(&h) {
            self.stats.dropped_dup += 1;
            return;
        }
        if self.pending_bytes() + pkt.len() > MAX_QUEUE_BYTES {
            self.stats.dropped_full += 1;
            return;
        }
        let pid = self.next_pid;
        self.next_pid = self.next_pid % 0xFFFF + 1;
        self.ranges.push_back((pid, 0, pkt.len(), 0));
        self.packets.insert(pid, pkt);
        self.waiting.insert(h, pid);
        self.stats.queued += 1;
    }

    pub fn pending_bytes(&self) -> usize {
        self.ranges.iter().map(|r| r.2).sum()
    }

    fn forget_unreferenced(&mut self) {
        let live: HashSet<u16> =
            self.ranges.iter().map(|r| r.0).chain(self.inflight.values().flatten().map(|f| f.1)).collect();
        self.packets.retain(|p, _| live.contains(p));
        let queued: HashSet<u16> = self.ranges.iter().filter(|r| r.1 == 0).map(|r| r.0).collect();
        self.waiting.retain(|_, p| queued.contains(p));
    }
}

impl Traffic for PacketQueue {
    fn fill(&mut self, seq: u16, caps: &[usize]) -> Vec<Vec<u8>> {
        let mut out = vec![vec![]; caps.len()];
        let mut sent = vec![];
        for tile in tile_order(caps, &mut self.rng) {
            let room = caps[tile];
            let mut block: Vec<u8> = vec![];
            while room > block.len() + FRAG_HDR {
                let Some((pid, off, n, tries)) = self.ranges.pop_front() else { break };
                let take = n.min(room - block.len() - FRAG_HDR);
                let pkt = &self.packets[&pid];
                block.extend_from_slice(&pid.to_be_bytes());
                block.extend_from_slice(&(off as u16).to_be_bytes());
                block.extend_from_slice(&(take as u16).to_be_bytes());
                block.extend_from_slice(&(pkt.len() as u16).to_be_bytes());
                block.extend_from_slice(&pkt[off..off + take]);
                sent.push((tile, pid, off, take, tries));
                if take < n {
                    self.ranges.push_front((pid, off + take, n - take, tries));
                }
            }
            out[tile] = block;
        }
        self.inflight.insert(seq, sent);
        self.forget_unreferenced();
        out
    }

    fn ack(&mut self, seq: u16, ok: &[bool]) {
        let mut retry = vec![];
        for (tile, pid, off, n, tries) in self.inflight.remove(&seq).unwrap_or_default() {
            if ok.get(tile).copied().unwrap_or(false) {
                continue;
            }
            if tries + 1 < MAX_TRIES {
                retry.push((pid, off, n, tries + 1));
            } else {
                self.stats.given_up += 1;
            }
        }
        for r in retry.into_iter().rev() {
            self.ranges.push_front(r); // retransmissions first, in order
        }
        self.forget_unreferenced();
    }

    fn requeue_before(&mut self, seq: u16) {
        let old: Vec<u16> = self.inflight.keys().copied().filter(|&s| s < seq).collect();
        for s in old {
            self.ack(s, &[]);
        }
    }
}

/// Parses fragments; complete packets are returned once.
#[derive(Default)]
pub struct PacketReassembly {
    partial: HashMap<(u16, usize), (Vec<u8>, Vec<bool>, Instant)>,
    recent: VecDeque<(u16, usize)>,
    pub delivered: u64,
    pub expired: u64,
}


impl PacketReassembly {
    pub fn put(&mut self, block: &[u8]) -> Vec<Vec<u8>> {
        let mut done = vec![];
        let mut i = 0;
        while i + FRAG_HDR <= block.len() {
            let rd = |o: usize| u16::from_be_bytes([block[i + o], block[i + o + 1]]) as usize;
            let pid = rd(0) as u16;
            if pid == 0 {
                break;
            }
            let (off, n, total) = (rd(2), rd(4), rd(6));
            let data = &block[i + FRAG_HDR..(i + FRAG_HDR + n).min(block.len())];
            i += FRAG_HDR + n;
            let key = (pid, total);
            if self.recent.contains(&key) || off + data.len() > total {
                continue;
            }
            let e = self.partial.entry(key).or_insert_with(|| (vec![0; total], vec![false; total], Instant::now()));
            e.0[off..off + data.len()].copy_from_slice(data);
            e.1[off..off + data.len()].iter_mut().for_each(|h| *h = true);
            if e.1.iter().all(|&h| h) {
                let (buf, _, _) = self.partial.remove(&key).unwrap();
                self.recent.push_back(key);
                if self.recent.len() > 2048 {
                    self.recent.pop_front();
                }
                self.delivered += 1;
                done.push(buf);
            }
        }
        let before = self.partial.len();
        self.partial.retain(|_, v| v.2.elapsed() < Duration::from_secs(180));
        self.expired += (before - self.partial.len()) as u64;
        done
    }
}
