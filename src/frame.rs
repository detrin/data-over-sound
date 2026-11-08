//! Link-layer framing on top of the OFDM PHY.
//!
//! - Each data tile carries one self-contained block: payload | CRC-16 | Reed-Solomon parity, so a
//!   tile succeeds or fails on its own (per-tile feedback for the adaptation).
//! - The header (DBPSK on all subcarriers, RS(255,64), byte-contiguous interleaving, two
//!   soft-combined copies) carries the levels of this frame plus feedback for the peer.

use crate::phy::{Phy, PhyConfig};
use crate::rs;

pub const CRC_BYTES: usize = 2;
pub const HEADER_INFO: usize = 64;
pub const NO_FEEDBACK: u16 = 0xFFFF;

fn crc16(b: &[u8]) -> [u8; 2] {
    let c = crc32fast::hash(b) & 0xFFFF;
    [(c >> 8) as u8, c as u8]
}

/// (capacity bytes, payload bytes, parity bytes) of one tile block at `level`.
pub fn block_layout(cfg: &PhyConfig, level: u8) -> (usize, usize, usize) {
    let cap = cfg.tile_bits(level) / 8;
    if cap == 0 {
        return (0, 0, 0);
    }
    let par = ((cap as f64 * cfg.rs_fraction).round_ties_even() as usize).max(2);
    (cap, cap - CRC_BYTES - par, par)
}

pub fn payload_capacity(cfg: &PhyConfig, level: u8) -> usize {
    block_layout(cfg, level).1
}

pub fn unpack_bits(bytes: &[u8]) -> Vec<u8> {
    bytes.iter().flat_map(|&b| (0..8).rev().map(move |i| (b >> i) & 1)).collect()
}

pub fn pack_bits(bits: &[u8]) -> Vec<u8> {
    bits.chunks(8).map(|ch| ch.iter().enumerate().fold(0u8, |a, (i, &b)| a | ((b & 1) << (7 - i)))).collect()
}

pub fn encode_block(cfg: &PhyConfig, level: u8, payload: &[u8]) -> Vec<u8> {
    let (_, n, par) = block_layout(cfg, level);
    let mut p = payload[..payload.len().min(n)].to_vec();
    p.resize(n, 0);
    let crc = crc16(&p);
    p.extend_from_slice(&crc);
    let mut bits = unpack_bits(&rs::encode(&p, par));
    bits.resize(cfg.tile_bits(level), 0);
    bits
}

pub fn decode_block(cfg: &PhyConfig, level: u8, bits: &[u8]) -> Option<Vec<u8>> {
    let (cap, n, par) = block_layout(cfg, level);
    let data = rs::decode(&pack_bits(&bits[..cap * 8]), par)?;
    let (payload, crc) = data.split_at(n);
    (crc16(payload) == crc).then(|| payload.to_vec())
}

#[derive(Clone, Debug, PartialEq)]
pub struct Header {
    pub seq: u16,
    /// levels used for the data tiles of THIS frame
    pub levels: Vec<u8>,
    /// peer frame the feedback refers to (NO_FEEDBACK = none)
    pub fb_seq: u16,
    /// per-tile success of that peer frame
    pub fb_ok: Vec<bool>,
    /// levels the peer should use next
    pub fb_levels: Vec<u8>,
}

impl Header {
    pub fn new(seq: u16, levels: Vec<u8>) -> Self {
        let n = levels.len();
        Self { seq, levels, fb_seq: NO_FEEDBACK, fb_ok: vec![false; n], fb_levels: vec![0; n] }
    }

    pub fn pack(&self, n_tiles: usize) -> Vec<u8> {
        let lv = |a: &[u8]| pack_bits(&a.iter().flat_map(|&l| [(l >> 1) & 1, l & 1]).collect::<Vec<u8>>());
        let mut b = Vec::with_capacity(HEADER_INFO);
        b.extend_from_slice(&self.seq.to_be_bytes());
        b.extend_from_slice(&self.fb_seq.to_be_bytes());
        b.extend(lv(&self.levels));
        b.extend(lv(&self.fb_levels));
        b.extend(pack_bits(&self.fb_ok.iter().map(|&o| o as u8).collect::<Vec<u8>>()));
        assert!(b.len() <= HEADER_INFO && self.levels.len() == n_tiles);
        b.resize(HEADER_INFO, 0);
        b
    }

    pub fn unpack(b: &[u8], n_tiles: usize) -> Self {
        let nl = (2 * n_tiles).div_ceil(8);
        let lv = |raw: &[u8]| -> Vec<u8> {
            let bits = unpack_bits(raw);
            (0..n_tiles).map(|i| bits[2 * i] * 2 + bits[2 * i + 1]).collect()
        };
        let ok_bits = unpack_bits(&b[4 + 2 * nl..4 + 2 * nl + n_tiles.div_ceil(8)]);
        Self {
            seq: u16::from_be_bytes([b[0], b[1]]),
            fb_seq: u16::from_be_bytes([b[2], b[3]]),
            levels: lv(&b[4..4 + nl]),
            fb_levels: lv(&b[4 + nl..4 + 2 * nl]),
            fb_ok: ok_bits[..n_tiles].iter().map(|&x| x == 1).collect(),
        }
    }
}

/// (copies x codeword bits) positions among the header subcarrier bits: each byte keeps its 8 bits
/// on adjacent subcarriers and the bytes are scattered over the band (narrowband interference hits
/// only the bytes in its band); every copy uses a different scattering.
fn header_positions(phy: &Phy) -> Vec<Vec<usize>> {
    let c = &phy.cfg;
    let nb = c.header_bytes();
    (0..c.header_copies())
        .map(|copy| {
            phy.tables.header_slots[copy * nb..(copy + 1) * nb].iter().flat_map(|&slot| (0..8).map(move |i| slot * 8 + i)).collect()
        })
        .collect()
}

pub fn encode_header(phy: &Phy, h: &Header) -> Vec<u8> {
    let c = &phy.cfg;
    let cw = rs::encode(&h.pack(c.n_tiles()), c.header_bytes() - HEADER_INFO);
    let bits = unpack_bits(&cw);
    let mut out = vec![0u8; c.header_bits()];
    for copy in header_positions(phy) {
        for (i, &p) in copy.iter().enumerate() {
            out[p] = bits[i];
        }
    }
    out
}

/// soft: reliability-weighted DBPSK metrics of all header subcarriers (> 0 means bit 0); copies are
/// summed before the hard decision and RS decoding.
pub fn decode_header(phy: &Phy, soft: &[f64]) -> Option<Header> {
    let c = &phy.cfg;
    let pos = header_positions(phy);
    let bits: Vec<u8> = (0..pos[0].len()).map(|i| (pos.iter().map(|copy| soft[copy[i]]).sum::<f64>() < 0.0) as u8).collect();
    let info = rs::decode(&pack_bits(&bits), c.header_bytes() - HEADER_INFO)?;
    Some(Header::unpack(&info, c.n_tiles()))
}
