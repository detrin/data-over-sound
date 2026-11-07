"""OFDM physical layer for an acoustic (speaker -> room -> mic) channel.

Frame: [sync chirp][gap][reference symbol][header symbols][data symbols]
- Long symbols (default 640 ms + 160 ms cyclic prefix): room reverb tails of ~100+ ms then only
  corrupt a small fraction of each FFT window.
- Differential PSK *in time* on every subcarrier: the uneven speaker/room frequency response and a
  constant timing offset cancel out; no channel equaliser is needed.
- The band is split into tiles (groups of adjacent subcarriers). Each tile gets a level:
  0 = pilot (known pseudo-random DQPSK, carries no data but is still measured),
  1 = DBPSK with 3x repetition (copies ~2 symbols apart, soft-combined): the robust fallback,
  2 = DQPSK, 3 = D8PSK.
- The receiver measures every tile's effective SNR from the phase error of the differential
  products (captures noise, interference, inter-symbol interference and clock drift at once).
"""
from dataclasses import dataclass
from functools import cached_property
from typing import Tuple

import numpy as np
from scipy.signal import chirp, fftconvolve

MAX_LEVEL = 3
L1_REPEAT = 3
GRAY = {1: np.array([0, 1]), 2: np.array([0, 1, 3, 2]), 3: np.array([0, 1, 3, 2, 6, 7, 5, 4])}


@dataclass(frozen=True)
class PhyConfig:
    # Defaults chosen by ofdm.sweep_phy on the measured room (reverb ~ -10 dB beyond 60 ms):
    # long symbols make most of the reverb part of the channel instead of interference.
    fs: int = 48000
    nfft: int = 30720           # 640 ms symbols -> 1.5625 Hz subcarrier spacing
    ncp: int = 7680             # 160 ms cyclic prefix
    f_lo: float = 800.0
    f_hi: float = 10000.0
    tile_sc: int = 64           # subcarriers per tile (100 Hz) -> 92 tiles
    n_header: int = 1
    n_data: int = 6
    chirp_s: float = 0.25          # long sync chirp: processing gain against narrowband noise
    gap_s: float = 0.15         # lets the chirp's reverb die before the reference symbol
    tx_rms: float = 0.12        # transmit RMS (OFDM is peaky; we soft-clip at tx_clip)
    tx_clip: float = 0.6
    rs_fraction: float = 0.25   # Reed-Solomon parity bytes per tile block

    @property
    def df(self) -> float:
        return self.fs / self.nfft

    @property
    def sym_len(self) -> int:
        return self.nfft + self.ncp

    @cached_property
    def bins(self) -> np.ndarray:
        b = np.arange(int(np.ceil(self.f_lo / self.df)), int(self.f_hi / self.df))
        return b[: len(b) // self.tile_sc * self.tile_sc]

    @property
    def n_tiles(self) -> int:
        return len(self.bins) // self.tile_sc

    @cached_property
    def tile_freqs(self) -> np.ndarray:
        return self.bins.reshape(self.n_tiles, self.tile_sc).mean(axis=1) * self.df

    @cached_property
    def sync(self) -> np.ndarray:
        t = np.arange(int(self.chirp_s * self.fs)) / self.fs
        return chirp(t, self.f_lo, t[-1], self.f_hi) * np.hanning(len(t))

    @property
    def n_syms(self) -> int:
        return 1 + self.n_header + self.n_data

    @property
    def frame_s(self) -> float:
        return (len(self.sync) + int(self.gap_s * self.fs) + self.n_syms * self.sym_len) / self.fs

    def tile_bits(self, level: int) -> int:
        if level == 1:
            return self.tile_sc * self.n_data // L1_REPEAT
        return level * self.tile_sc * self.n_data

    @property
    def header_bits(self) -> int:
        """Header subcarrier bits (DBPSK on every subcarrier of the header symbols)."""
        return len(self.bins) * self.n_header

    @property
    def header_bytes(self) -> int:
        """Header RS codeword length: limited to one RS(255) codeword."""
        return min(self.header_bits // 8, 255)

    @property
    def header_copies(self) -> int:
        """How many copies of the header codeword fit on the header symbols (soft-combined at RX)."""
        return max(1, (self.header_bits // 8) // self.header_bytes)


def _rng_phases(cfg: PhyConfig, salt: int, shape) -> np.ndarray:
    return np.random.default_rng(1000 + salt).integers(0, 4, size=shape)


def reference(cfg: PhyConfig) -> np.ndarray:
    return np.exp(1j * (np.pi / 2 * _rng_phases(cfg, 0, len(cfg.bins)) + np.pi / 4))


def pilot_steps(cfg: PhyConfig) -> np.ndarray:
    """Known DQPSK phase steps (indices 0..3) for level-0 tiles, per data symbol and subcarrier."""
    return _rng_phases(cfg, 1, (cfg.n_data, len(cfg.bins)))


def _psk_steps(bits: np.ndarray, level: int) -> np.ndarray:
    """bits (n * level) -> Gray-mapped symbol indices 0..2^level-1."""
    groups = bits.reshape(-1, level)
    idx = groups @ (1 << np.arange(level - 1, -1, -1))
    return GRAY[level][idx]


def _psk_bits(steps: np.ndarray, level: int) -> np.ndarray:
    inv = np.argsort(GRAY[level])
    idx = inv[steps]
    return ((idx[:, None] >> np.arange(level - 1, -1, -1)) & 1).astype(np.uint8).ravel()


def _l1_positions(cfg: PhyConfig) -> np.ndarray:
    """(copies, bits) positions inside a tile's (symbol-major) data grid for repeated DBPSK.
    Copy r of bit i sits n positions later (n = bits per tile), i.e. ~2 symbols apart."""
    n = cfg.tile_bits(1)
    return np.arange(n)[None, :] + n * np.arange(L1_REPEAT)[:, None]


def modulate(cfg: PhyConfig, header_bits: np.ndarray, levels: np.ndarray,
             tile_bits: list) -> np.ndarray:
    """Build one frame. header_bits: cfg.header_bits bits; levels: per-tile level;
    tile_bits[k]: cfg.tile_bits(levels[k]) bits for tile k (ignored for level 0)."""
    nb, ts = len(cfg.bins), cfg.tile_sc
    phase_steps = np.zeros((cfg.n_syms - 1, nb))                      # radians, per symbol after ref
    phase_steps[: cfg.n_header] = np.pi * header_bits.reshape(cfg.n_header, nb)
    pil = pilot_steps(cfg)
    for k, lev in enumerate(levels):
        sl = slice(k * ts, (k + 1) * ts)
        if lev == 0:
            phase_steps[cfg.n_header:, sl] = np.pi / 2 * pil[:, sl]
        elif lev == 1:
            grid = np.zeros(cfg.n_data * ts)
            grid[_l1_positions(cfg)] = np.pi * np.asarray(tile_bits[k], dtype=np.uint8)[None, :]
            phase_steps[cfg.n_header:, sl] = grid.reshape(cfg.n_data, ts)
        else:
            # bit order: symbol-major, then subcarrier, then the level bits of that subcarrier
            steps = _psk_steps(np.asarray(tile_bits[k], dtype=np.uint8), lev).reshape(cfg.n_data, ts)
            phase_steps[cfg.n_header:, sl] = 2 * np.pi / (1 << lev) * steps
    X = np.empty((cfg.n_syms, nb), dtype=complex)
    X[0] = reference(cfg)
    X[1:] = X[0] * np.exp(1j * np.cumsum(phase_steps, axis=0))
    spec = np.zeros((cfg.n_syms, cfg.nfft // 2 + 1), dtype=complex)
    spec[:, cfg.bins] = X
    body = np.fft.irfft(spec, cfg.nfft, axis=1)
    body = np.concatenate([body[:, -cfg.ncp:], body], axis=1).ravel()
    body *= cfg.tx_rms / np.sqrt(np.mean(body ** 2))
    body = cfg.tx_clip * np.tanh(body / cfg.tx_clip)                 # soft clip the OFDM peaks
    sync = cfg.sync * (cfg.tx_rms * 3)
    return np.concatenate([sync, np.zeros(int(cfg.gap_s * cfg.fs)), body])


def find_sync(cfg: PhyConfig, rx: np.ndarray) -> int:
    """Index of the first sample after the chirp (direct path), or -1."""
    c = np.abs(fftconvolve(rx, cfg.sync[::-1], mode="valid"))
    peak = int(np.argmax(c))
    if c[peak] < 6 * np.median(c):
        return -1
    lo = max(0, peak - int(0.03 * cfg.fs))                            # earliest strong arrival
    early = lo + int(np.argmax(c[lo:peak + 1] > 0.5 * c[peak]))
    return early + len(cfg.sync)


@dataclass
class Demod:
    header_bits: np.ndarray          # hard DBPSK decisions on all header subcarriers
    tile_bits: list
    tile_snr_db: np.ndarray          # effective SNR per tile from the data symbols (folds at high order)
    drift_ppm: float
    tile_snr_hdr_db: np.ndarray      # per-tile SNR from the DBPSK header symbol (all tiles, unfolded)
    header_soft: np.ndarray = None   # reliability-weighted soft DBPSK metrics (>0 means bit 0)


def demodulate(cfg: PhyConfig, rx: np.ndarray, levels: np.ndarray, start: int = None) -> Demod:
    start = find_sync(cfg, rx) if start is None else start
    if start < 0:
        raise ValueError("no sync")
    s0 = start + int(cfg.gap_s * cfg.fs) + cfg.ncp - cfg.ncp // 8    # FFT windows slightly early
    need = s0 + cfg.n_syms * cfg.sym_len
    if need > len(rx):
        rx = np.concatenate([rx, np.zeros(need - len(rx))])
    idx = s0 + np.arange(cfg.n_syms)[:, None] * cfg.sym_len + np.arange(cfg.nfft)[None, :]
    Y = np.fft.rfft(rx[idx], axis=1)[:, cfg.bins]
    z = Y[1:] * np.conj(Y[:-1])                                       # differential products
    nb, ts = len(cfg.bins), cfg.tile_sc
    f = cfg.bins * cfg.df

    # constellation order per subcarrier for the data symbols (pilots: known steps)
    levels = np.asarray(levels, dtype=int)
    order = np.repeat(levels, ts)
    pil = pilot_steps(cfg)

    def decide(ang):
        """Hard decisions -> (step indices, residual phase) for header (BPSK) + data."""
        steps = np.zeros(ang.shape, dtype=int)
        res = np.zeros(ang.shape)
        q = np.pi * np.round(ang[: cfg.n_header] / np.pi)
        steps[: cfg.n_header] = np.mod(np.round(q / np.pi), 2)
        res[: cfg.n_header] = _wrap(ang[: cfg.n_header] - q)
        a = ang[cfg.n_header:]
        for lev in range(0, MAX_LEVEL + 1):
            cols = order == lev
            if not cols.any():
                continue
            if lev == 0:
                ref = np.pi / 2 * pil[:, cols]
                res[cfg.n_header:, cols] = _wrap(a[:, cols] - ref)
                steps[cfg.n_header:, cols] = pil[:, cols]
            else:
                m = 1 << lev
                st = np.mod(np.round(a[:, cols] / (2 * np.pi / m)), m).astype(int)
                steps[cfg.n_header:, cols] = st
                res[cfg.n_header:, cols] = _wrap(a[:, cols] - 2 * np.pi / m * st)
        return steps, res

    ang = np.angle(z)
    steps, res = decide(ang)
    # sample-clock drift => phase step grows linearly with frequency; fit it on confident points
    w = np.abs(z) * (np.abs(res) < np.pi / 8)
    slope = (w * res * f).sum() / max((w * f * f).sum(), 1e-12)
    drift_ppm = slope / (2 * np.pi * cfg.sym_len / cfg.fs) * 1e6
    ang = ang - slope * f
    steps, res = decide(ang)
    zc = z * np.exp(-1j * slope * f)

    # soft DBPSK metric per (symbol, subcarrier), weighted by the measured reliability of that
    # symbol in that tile (1 / phase-error variance): jammed or burst-hit copies count little
    var = (res ** 2).reshape(cfg.n_syms - 1, cfg.n_tiles, ts).mean(axis=2)
    weight = np.repeat(1 / np.maximum(var, 1e-3), ts, axis=1)
    soft = np.real(zc) / np.maximum(np.abs(zc), 1e-30) * weight

    header = steps[: cfg.n_header].astype(np.uint8).ravel()
    tile_bits = []
    l1pos = _l1_positions(cfg)
    for k, lev in enumerate(levels):
        sl = slice(k * ts, (k + 1) * ts)
        if lev == 1:
            combined = soft[cfg.n_header:, sl].ravel()[l1pos].sum(axis=0)
            tile_bits.append((combined < 0).astype(np.uint8))
        else:
            tile_bits.append(_psk_bits(steps[cfg.n_header:, sl].ravel(), lev) if lev else np.zeros(0, np.uint8))
    # differential detection: phase-error variance ~ 1/SNR (two noisy symbols)
    # tiles with (almost) no energy (window past the recording, dead band) must not look clean:
    # a zero signal has zero phase error
    p = (np.abs(Y) ** 2).reshape(cfg.n_syms, cfg.n_tiles, cfg.tile_sc).mean(axis=(0, 2))
    dead = p < 1e-3 * max(np.median(p), 1e-30)
    if not np.any(p > 1e-20):
        dead[:] = True
    snr = np.where(dead, -10.0, _tile_snr(res[cfg.n_header:], cfg))
    # header symbols are DBPSK on every subcarrier: an unfolded SNR estimate for *every* tile,
    # whatever level the data map gives it (this is the free, every-frame band exploration)
    snr_hdr = np.where(dead, -10.0, _tile_snr(res[: cfg.n_header], cfg))
    return Demod(header, tile_bits, snr, drift_ppm, snr_hdr, soft[: cfg.n_header].ravel())


def _tile_snr(res: np.ndarray, cfg: PhyConfig) -> np.ndarray:
    var = (res ** 2).reshape(res.shape[0], cfg.n_tiles, cfg.tile_sc).mean(axis=(0, 2))
    return 10 * np.log10(1 / np.maximum(var, 1e-4))


def _wrap(a):
    return (a + np.pi) % (2 * np.pi) - np.pi
