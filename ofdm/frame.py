"""Link-layer framing on top of the OFDM PHY.

- Each data tile carries one self-contained block: payload | CRC-16 | Reed-Solomon parity.
  A tile therefore succeeds or fails on its own, which gives the adaptation per-tile feedback.
- The header (DBPSK on all subcarriers, heavy RS, bits interleaved across frequency so a narrowband
  interferer only hits a few bytes) carries the levels used in this frame plus feedback for the peer.
"""
import struct
import zlib
from dataclasses import dataclass, field
from typing import List, Optional

import numpy as np
import reedsolo

from .phy import PhyConfig

CRC_BYTES = 2
HEADER_INFO = 64            # bytes of header content (padded); fits up to 100 tiles


def _crc16(b: bytes) -> bytes:
    return struct.pack("!H", zlib.crc32(b) & 0xFFFF)


def block_layout(cfg: PhyConfig, level: int):
    """(capacity bytes, payload bytes, parity bytes) of one tile block at `level`."""
    cap = cfg.tile_bits(level) // 8
    if cap == 0:
        return 0, 0, 0
    par = max(2, int(round(cap * cfg.rs_fraction)))
    return cap, cap - CRC_BYTES - par, par


def encode_block(cfg: PhyConfig, level: int, payload: bytes) -> np.ndarray:
    cap, n, par = block_layout(cfg, level)
    payload = payload.ljust(n, b"\0")[:n]
    cw = bytes(reedsolo.RSCodec(par).encode(payload + _crc16(payload)))
    bits = np.unpackbits(np.frombuffer(cw, np.uint8))
    return np.concatenate([bits, np.zeros(cfg.tile_bits(level) - len(bits), np.uint8)])


def decode_block(cfg: PhyConfig, level: int, bits: np.ndarray) -> Optional[bytes]:
    cap, n, par = block_layout(cfg, level)
    cw = np.packbits(bits[: cap * 8]).tobytes()
    try:
        data = bytes(reedsolo.RSCodec(par).decode(cw)[0])
    except reedsolo.ReedSolomonError:
        return None
    payload, crc = data[:n], data[n:]
    return payload if _crc16(payload) == crc else None


@dataclass
class Header:
    seq: int
    levels: np.ndarray               # levels used for the data tiles of THIS frame
    fb_seq: int = 0xFFFF             # peer frame the feedback refers to (0xFFFF = none)
    fb_ok: np.ndarray = None         # per-tile success of that peer frame
    fb_levels: np.ndarray = None     # levels the peer should use next

    def pack(self, n_tiles: int) -> bytes:
        def lv(a):
            a = np.zeros(n_tiles, int) if a is None else np.asarray(a)
            return np.packbits(np.unpackbits(a.astype(np.uint8)[:, None], axis=1)[:, -2:].ravel()).tobytes()
        ok = np.zeros(n_tiles, np.uint8) if self.fb_ok is None else np.asarray(self.fb_ok, np.uint8)
        body = struct.pack("!HH", self.seq, self.fb_seq) + lv(self.levels) + lv(self.fb_levels) + np.packbits(ok).tobytes()
        assert len(body) <= HEADER_INFO, len(body)
        return body.ljust(HEADER_INFO, b"\0")

    @classmethod
    def unpack(cls, b: bytes, n_tiles: int) -> "Header":
        nl = (2 * n_tiles + 7) // 8
        def lv(raw):
            bits = np.unpackbits(np.frombuffer(raw, np.uint8))[: 2 * n_tiles].reshape(n_tiles, 2)
            return (bits[:, 0] * 2 + bits[:, 1]).astype(int)
        seq, fb_seq = struct.unpack("!HH", b[:4])
        levels, fb_levels = lv(b[4:4 + nl]), lv(b[4 + nl:4 + 2 * nl])
        ok = np.unpackbits(np.frombuffer(b[4 + 2 * nl:4 + 2 * nl + (n_tiles + 7) // 8], np.uint8))[:n_tiles]
        return cls(seq, levels, fb_seq, ok.astype(bool), fb_levels)


def _header_perm(cfg: PhyConfig) -> np.ndarray:
    """(copies, codeword bits) positions among the header subcarrier bits.

    RS corrects whole bytes, so each byte keeps its 8 bits on 8 adjacent subcarriers and the *bytes*
    are scattered over the band: a narrowband interferer then only destroys the bytes in its band.
    Every copy of the codeword uses a different scattering (frequency diversity)."""
    nb = cfg.header_bytes
    slots = np.random.default_rng(7).permutation(cfg.header_bits // 8)[: nb * cfg.header_copies]
    pos = (slots[:, None] * 8 + np.arange(8)[None, :]).ravel()
    return pos.reshape(cfg.header_copies, nb * 8)


def encode_header(cfg: PhyConfig, h: Header) -> np.ndarray:
    cw = bytes(reedsolo.RSCodec(cfg.header_bytes - HEADER_INFO).encode(h.pack(cfg.n_tiles)))
    out = np.zeros(cfg.header_bits, np.uint8)
    out[_header_perm(cfg)] = np.unpackbits(np.frombuffer(cw, np.uint8))[None, :]
    return out


def decode_header(cfg: PhyConfig, soft: np.ndarray) -> Optional[Header]:
    """soft: reliability-weighted DBPSK metrics of all header subcarriers (>0 = bit 0);
    the copies are summed (maximal-ratio style) before the hard decision and RS decoding."""
    combined = soft[_header_perm(cfg)].sum(axis=0)
    bits = (combined < 0).astype(np.uint8)
    try:
        info = bytes(reedsolo.RSCodec(cfg.header_bytes - HEADER_INFO).decode(np.packbits(bits).tobytes())[0])
    except reedsolo.ReedSolomonError:
        return None
    return Header.unpack(info, cfg.n_tiles)


def frame_payload_capacity(cfg: PhyConfig, levels) -> int:
    return sum(block_layout(cfg, int(l))[1] for l in levels)
