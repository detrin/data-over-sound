"""Over-the-air comparison of link-adaptation strategies with real broadcast noise.

The receiving laptop plays the scenario's interference through its own speaker while recording the
peer's frames. Strategies run round-robin frame by frame inside each noise phase, so they face the
same conditions. Feedback goes straight to each controller (the in-band feedback path is exercised
by ofdm.duplex).
"""
import argparse
import json
import os
import time
from collections import defaultdict

import numpy as np
from scipy.signal import butter, sosfiltfilt

from . import air, frame as fr
from .channel import Interferer
from .controllers import Fixed, Minstrel, SnrExpected, ThompsonSW, finalists
from .link import build_frame, receive_frame
from .phy import PhyConfig
from .scenario import PHASES

OUT = os.path.join(os.path.dirname(__file__), "data")


def band_rms(x, cfg):
    sos = butter(4, [cfg.f_lo, cfg.f_hi], btype="band", fs=cfg.fs, output="sos")
    return float(np.sqrt(np.mean(sosfiltfilt(sos, x) ** 2)))


def calibrate(tx, cfg, rng):
    """(received signal rms, receiver's self-noise gain) for direction tx -> peer."""
    levels = np.full(cfg.n_tiles, 2)
    wave, _ = build_frame(cfg, fr.Header(seq=0, levels=levels), levels, lambda n: rng.bytes(n))
    rec = air.transmit(tx, wave)
    sig = band_rms(rec[int(1.4 * cfg.fs): int((1.4 + cfg.frame_s - 0.5) * cfg.fs)], cfg)
    probe_rms = 0.01
    noise = Interferer(cfg.f_lo, cfg.f_hi, 0).render(int(4 * cfg.fs), cfg.fs, probe_rms, rng)
    rec = air.transmit(tx, np.zeros(int(2 * cfg.fs)), interferer=noise)
    self_gain = band_rms(rec[cfg.fs: 3 * cfg.fs], cfg) / probe_rms
    return sig, self_gain


def make_strategies(cfg, seed=0):
    return [ThompsonSW(cfg, gamma=0.7, snr_prior=True, seed=seed), SnrExpected(cfg, seed=seed),
            Minstrel(cfg, seed=seed), Fixed(cfg, 3, seed=seed)]


def run_direction(tx, cfg, frames_per_phase, log, seed=0, strategies=None):
    rng = np.random.default_rng(seed)
    src = lambda n: rng.bytes(n)
    sig, self_gain = calibrate(tx, cfg, rng)
    print(f"[{tx}] received signal rms {sig:.4f}, self-noise gain {self_gain:.2f}", file=log, flush=True)
    strategies = strategies or make_strategies(cfg, seed)
    bits = defaultdict(lambda: np.zeros(len(PHASES)))
    frames = defaultdict(list)                       # per strategy: [(phase, rep, bit/s)]
    lost = defaultdict(int)
    seq = 0
    noise_len = int((cfg.frame_s + 3) * cfg.fs)
    for pi, (pname, its) in enumerate(PHASES):
        for rep in range(frames_per_phase):
            for ctrl in strategies:
                levels = ctrl.choose()
                wave, pl = build_frame(cfg, fr.Header(seq=seq, levels=levels), levels, src)
                noise = None
                if its:
                    # render at the SIR the scenario asks for, then undo the self-path gain
                    noise = sum(it.render(noise_len, cfg.fs, sig / self_gain, rng) for it in its)
                rec = air.transmit(tx, wave, interferer=noise)
                r = receive_frame(cfg, rec, sent_payloads=pl)
                ctrl.observe(levels, r.tile_ok, r.tile_snr_db)
                bits[ctrl.name][pi] += 8 * r.payload_bytes
                frames[ctrl.name].append((pi, rep, 8 * r.payload_bytes / cfg.frame_s))
                lost[ctrl.name] += r.header is None
                print(f"[{tx}] {pname:16s} #{rep} {ctrl.name:18s} levels {''.join(map(str, levels))} "
                      f"ok {r.tile_ok.sum():2d}/{cfg.n_tiles} {8 * r.payload_bytes / cfg.frame_s:6.0f} bit/s "
                      f"hdr {'ok' if r.header else 'LOST'} snr med {np.nanmedian(r.tile_snr_db):5.1f}",
                      file=log, flush=True)
                seq += 1
    airtime = frames_per_phase * cfg.frame_s
    return {name: {"per_phase": (b / airtime).tolist(), "mean": float((b / airtime).mean()),
                   "lost": lost[name], "frames": frames[name]} for name, b in bits.items()}


def paired_vs(results, baseline):
    """Mean and standard error of the per-frame goodput difference to `baseline`; frames are
    paired by (phase, repetition), i.e. sent back to back under the same broadcast noise."""
    base = {(p, r): v for p, r, v in results[baseline]["frames"]}
    out = {}
    for name, res in results.items():
        if name == baseline:
            continue
        d = np.array([v - base[(p, r)] for p, r, v in res["frames"]])
        out[name] = (float(d.mean()), float(d.std(ddof=1) / np.sqrt(len(d))))
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--frames-per-phase", type=int, default=4)
    ap.add_argument("--finalists", action="store_true", help="round 2: fixed-L3 vs Thompson with/without online calibration")
    ap.add_argument("--tag", default="air_compare")
    args = ap.parse_args()
    cfg = PhyConfig()
    t = time.time()
    results = {}
    with open(os.path.join(OUT, f"{args.tag}.log"), "w") as log:
        for tx in air.HOSTS:
            strategies = [c for c in finalists(cfg) if not c.name.startswith("snr-expected")] if args.finalists else None
            results[f"{tx}_to_{air.peer(tx)}"] = run_direction(tx, cfg, args.frames_per_phase, log, strategies=strategies)
            with open(os.path.join(OUT, f"{args.tag}.json"), "w") as f:
                json.dump(results, f, indent=1)
    print(f"done in {(time.time() - t) / 60:.0f} min")
    for d, res in results.items():
        print("==", d)
        print(f"{'strategy':20s}" + "".join(f"{n[:14]:>15s}" for n, _ in PHASES) + f"{'MEAN':>9s}{'lost':>6s}")
        for name, r in sorted(res.items(), key=lambda kv: -kv[1]["mean"]):
            print(f"{name:20s}" + "".join(f"{v:15.0f}" for v in r["per_phase"]) + f"{r['mean']:9.0f}{r['lost']:6d}")
        if "fixed-L3" in res:
            for name, (m, se) in paired_vs(res, "fixed-L3").items():
                print(f"   {name} - fixed-L3: {m:+.0f} +/- {se:.0f} bit/s per frame (paired, 1 s.e.)")


if __name__ == "__main__":
    main()
