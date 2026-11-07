"""Measure each direction's acoustic impulse response (exponential sine sweep, Farina) and noise PSD.

Saves data/channel_<tx>_to_<rx>.npz with `ir` (normalised, starting at the direct path) and
`noise_psd` (Welch, per Hz) for the channel simulator.
"""
import os
import numpy as np
from scipy.signal import welch, fftconvolve

from . import air

F1, F2, T = 200.0, 12000.0, 3.0
AMP = 0.2
OUT = os.path.join(os.path.dirname(__file__), "data")


def ess(fs=air.FS):
    t = np.arange(int(T * fs)) / fs
    k = T / np.log(F2 / F1)
    sweep = np.sin(2 * np.pi * F1 * k * (np.exp(t / k) - 1))
    fade = np.minimum(1, np.minimum(t / 0.05, (T - t) / 0.05))
    inverse = sweep[::-1] * np.exp(-t / k)            # amplitude-compensated time reverse
    return AMP * sweep * fade, inverse


def impulse_response(rec, inverse):
    h = fftconvolve(rec, inverse)
    peak = int(np.argmax(np.abs(h)))
    ir = h[peak - 200: peak + int(0.8 * air.FS)]
    return ir / np.max(np.abs(ir))


def delay_spread(ir, fs=air.FS):
    """Time after the direct path until the energy envelope falls 20 dB and 30 dB below the direct path,
    plus RMS delay spread."""
    e = ir[200:] ** 2
    env = 10 * np.log10(np.convolve(e, np.ones(48) / 48, mode="same") / e.max() + 1e-12)
    t = np.arange(len(e)) / fs
    def last_above(db):
        idx = np.where(env > db)[0]
        return t[idx[-1]] if len(idx) else 0.0
    p = e / e.sum()
    tau = (p * t).sum(); rms = np.sqrt((p * (t - tau) ** 2).sum())
    return last_above(-20), last_above(-30), rms


def isi_fraction(ir, guards_ms=(2, 5, 10, 15, 20, 30, 40, 60), fs=air.FS):
    """Energy arriving later than each guard interval, relative to the energy inside it (dB).
    With a cyclic prefix of that length, this is roughly the inter-symbol interference floor."""
    e = ir[200:] ** 2
    floor = e[int(0.5 * fs):].mean()                  # deconvolution noise, measured after the reverb died
    e = np.maximum(e[: int(0.5 * fs)] - floor, 0)
    out = {}
    for g in guards_ms:
        n = int(g * fs / 1000)
        out[g] = 10 * np.log10(e[n:].sum() / e[:n].sum() + 1e-12)
    return out


def main():
    os.makedirs(OUT, exist_ok=True)
    sweep, inverse = ess()
    for tx in air.HOSTS:
        rx = air.peer(tx)
        rec = air.transmit(tx, sweep)
        ir = impulse_response(rec, inverse)
        noise = air.record_silence(rx, 4)
        f, psd = welch(noise, air.FS, nperseg=4096)
        t20, t30, rms = delay_spread(ir)
        # absolute gain: received sweep level vs the sweep convolved with the (peak-normalised) IR
        sim = fftconvolve(sweep, ir)
        h = fftconvolve(rec, inverse); peak = int(np.argmax(np.abs(h)))
        seg = rec[max(0, peak - len(sweep)): peak + int(0.3 * air.FS)]
        gain = np.sqrt(np.mean(seg ** 2) / np.mean(sim[: len(seg)] ** 2))
        np.savez(f"{OUT}/channel_{tx}_to_{rx}.npz", ir=ir, gain=gain, noise_psd=psd, noise_f=f,
                 rec_peak=np.max(np.abs(rec)), noise_rms=np.sqrt(np.mean(noise ** 2)))
        print(f"{tx} -> {rx}: rec peak {np.max(np.abs(rec)):.2f}, noise rms {np.sqrt(np.mean(noise**2)):.4f}, "
              f"echo -20dB after {1e3*t20:.1f} ms, -30dB after {1e3*t30:.1f} ms")
        print("   ISI floor (late/early energy, dB) per guard ms:",
              " ".join(f"{g}:{v:.0f}" for g, v in isi_fraction(ir).items()))


if __name__ == "__main__":
    main()
