//! One-frame transmit/receive shared by simulation, the file transfer and the network card.

use crate::frame::{self, Header};
use crate::phy::Phy;

pub struct RxResult {
    pub header: Option<Header>,
    /// per tile: block decoded (false for pilots)
    pub tile_ok: Vec<bool>,
    /// per-tile SNR from the DBPSK header symbol (every tile, every frame); NaN if no sync
    pub tile_snr_db: Vec<f64>,
    /// correctly received payload bytes
    pub payload_bytes: usize,
    pub drift_ppm: f64,
    /// decoded payload per tile
    pub blocks: Vec<Option<Vec<u8>>>,
}

/// Build a frame; `payload(n)` supplies n payload bytes per data tile. Returns (waveform, payloads).
pub fn build_frame(phy: &Phy, header: &Header, levels: &[u8], mut payload: impl FnMut(usize) -> Vec<u8>) -> (Vec<f64>, Vec<Vec<u8>>) {
    let mut payloads = Vec::with_capacity(levels.len());
    let mut bits = Vec::with_capacity(levels.len());
    for &lev in levels {
        let n = frame::payload_capacity(&phy.cfg, lev);
        let p = if n > 0 { payload(n) } else { vec![] };
        bits.push(if lev > 0 { frame::encode_block(&phy.cfg, lev, &p) } else { vec![] });
        payloads.push(p);
    }
    let hb = frame::encode_header(phy, header);
    (phy.modulate(&hb, levels, &bits), payloads)
}

/// Decode a frame. Levels come from the header unless given; `sent` (if given) verifies payloads.
pub fn receive_frame(phy: &Phy, rx: &[f64], levels: Option<&[u8]>, sent: Option<&[Vec<u8>]>) -> RxResult {
    let nt = phy.cfg.n_tiles();
    let lost = |snr: Vec<f64>, drift: f64| RxResult {
        header: None,
        tile_ok: vec![false; nt],
        tile_snr_db: snr,
        payload_bytes: 0,
        drift_ppm: drift,
        blocks: vec![None; nt],
    };
    let Some(start) = phy.find_sync(rx) else {
        return lost(vec![f64::NAN; nt], f64::NAN);
    };
    let first = phy.demodulate(rx, &vec![0u8; nt], Some(start)).expect("demod");
    let header = frame::decode_header(phy, &first.header_soft);
    let levels: Vec<u8> = match (levels, &header) {
        (Some(l), _) => l.to_vec(),
        (None, Some(h)) => h.levels.clone(),
        (None, None) => return lost(first.tile_snr_db, first.drift_ppm),
    };
    let d = phy.demodulate(rx, &levels, Some(start)).expect("demod");
    let mut ok = vec![false; nt];
    let mut blocks = vec![None; nt];
    let mut good = 0;
    for (k, &lev) in levels.iter().enumerate() {
        if lev == 0 {
            continue;
        }
        if let Some(p) = frame::decode_block(&phy.cfg, lev, &d.tile_bits[k]) {
            if sent.is_none_or(|s| s[k] == p) {
                ok[k] = true;
                good += p.len();
                blocks[k] = Some(p);
            }
        }
    }
    RxResult { header, tile_ok: ok, tile_snr_db: d.tile_snr_db, payload_bytes: good, drift_ppm: d.drift_ppm, blocks }
}
