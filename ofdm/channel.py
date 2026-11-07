"""Acoustic channel simulator calibrated from measurements (ofdm/data/channel_*.npz).

rx = gain * (tx * room IR) resampled by the sound-card clock offset + measured background noise
     + time-varying interferers (the "changing noise" the link has to adapt to).
"""
import os
from dataclasses import dataclass
from typing import List, Optional

import numpy as np
from scipy.signal import fftconvolve

DATA = os.path.join(os.path.dirname(__file__), "data")


@dataclass
class Interferer:
    """Band-limited Gaussian noise at a given level relative to the received signal (SIR, dB)."""
    f_lo: float
    f_hi: float
    sir_db: float
    duty: float = 1.0          # <1: gated bursts (fraction of time on), 0.25 s granularity

    def render(self, n: int, fs: int, ref_rms: float, rng) -> np.ndarray:
        spec = np.zeros(n // 2 + 1, complex)
        f = np.fft.rfftfreq(n, 1 / fs)
        band = (f >= self.f_lo) & (f <= self.f_hi)
        spec[band] = rng.normal(size=band.sum()) + 1j * rng.normal(size=band.sum())
        x = np.fft.irfft(spec, n)
        x *= ref_rms * 10 ** (-self.sir_db / 20) / (np.sqrt(np.mean(x ** 2)) + 1e-12)
        if self.duty < 1:
            seg = int(0.25 * fs)
            gate = np.repeat(rng.random(n // seg + 1) < self.duty, seg)[:n]
            x *= gate / np.sqrt(self.duty)
        return x


class AcousticChannel:
    def __init__(self, name: str, drift_ppm: float = 1.0, seed: int = 0):
        d = np.load(os.path.join(DATA, f"channel_{name}.npz"))
        self.ir, self.gain = d["ir"], float(d["gain"])
        self.noise_f, self.noise_psd, self.noise_rms = d["noise_f"], d["noise_psd"], float(d["noise_rms"])
        self.drift = drift_ppm * 1e-6
        self.rng = np.random.default_rng(seed)

    def _noise(self, n: int, fs: int) -> np.ndarray:
        f = np.fft.rfftfreq(n, 1 / fs)
        shape = np.sqrt(np.interp(f, self.noise_f, self.noise_psd))
        spec = (self.rng.normal(size=len(f)) + 1j * self.rng.normal(size=len(f))) * shape
        x = np.fft.irfft(spec, n)
        return x * self.noise_rms / (np.sqrt(np.mean(x ** 2)) + 1e-12)

    def __call__(self, tx: np.ndarray, fs: int = 48000, interferers: Optional[List[Interferer]] = None,
                 lead_s: float = 0.5) -> np.ndarray:
        y = self.gain * fftconvolve(np.concatenate([np.zeros(int(lead_s * fs)), tx, np.zeros(fs // 2)]), self.ir)
        if self.drift:
            t = np.arange(len(y))
            y = np.interp(t * (1 + self.drift), t, y)
        sig_rms = np.sqrt(np.mean(y[int(lead_s * fs): int(lead_s * fs) + len(tx)] ** 2))
        y = y + self._noise(len(y), fs)
        for it in interferers or []:
            y = y + it.render(len(y), fs, sig_rms, self.rng)
        return y
