"""One-frame transmit/receive helpers shared by simulation and over-the-air runs."""
from dataclasses import dataclass
from typing import List, Optional

import numpy as np

from . import frame as fr
from .phy import PhyConfig, demodulate, modulate


@dataclass
class RxResult:
    header: Optional[fr.Header]
    tile_ok: np.ndarray              # per tile: True/False for data tiles (False for pilots)
    tile_snr_db: np.ndarray          # per-tile SNR from the DBPSK header symbol (every tile, every frame)
    payload_bytes: int               # correctly received payload bytes
    drift_ppm: float
    tile_snr_data_db: np.ndarray = None
    blocks: list = None              # decoded payload per tile (None where the block failed)


def build_frame(cfg: PhyConfig, header: fr.Header, levels, payload_source) -> (np.ndarray, list):
    """payload_source(n) -> n bytes. Returns (waveform, list of per-tile payloads)."""
    levels = np.asarray(levels)
    payloads, tile_bits = [], []
    for lev in levels:
        n = fr.block_layout(cfg, int(lev))[1]
        p = payload_source(n) if n else b""
        payloads.append(p)
        tile_bits.append(fr.encode_block(cfg, int(lev), p) if lev else None)
    hb = fr.encode_header(cfg, header)
    return modulate(cfg, hb, levels, tile_bits), payloads


def receive_frame(cfg: PhyConfig, rx: np.ndarray, levels=None, sent_payloads=None) -> RxResult:
    """Decode a frame. Levels come from the header unless given; sent_payloads (if given) are used
    to verify the delivered data byte-for-byte."""
    try:
        first = demodulate(cfg, rx, np.zeros(cfg.n_tiles, int))   # header + sync only
    except ValueError:                                            # no sync: the whole frame is lost
        nan = np.full(cfg.n_tiles, np.nan)
        return RxResult(None, np.zeros(cfg.n_tiles, bool), nan, 0, float("nan"), nan)
    header = fr.decode_header(cfg, first.header_soft)
    if levels is None:
        if header is None:
            return RxResult(None, np.zeros(cfg.n_tiles, bool), first.tile_snr_hdr_db, 0, first.drift_ppm)
        levels = header.levels
    levels = np.asarray(levels, dtype=int)
    d = demodulate(cfg, rx, levels)
    ok = np.zeros(cfg.n_tiles, bool); good = 0
    blocks = [None] * cfg.n_tiles
    for k, lev in enumerate(levels):
        if not lev:
            continue
        p = fr.decode_block(cfg, int(lev), d.tile_bits[k])
        ok[k] = p is not None and (sent_payloads is None or p == sent_payloads[k])
        if ok[k]:
            good += len(p)
            blocks[k] = p
    return RxResult(header, ok, d.tile_snr_hdr_db, good, d.drift_ppm, d.tile_snr_db, blocks)
