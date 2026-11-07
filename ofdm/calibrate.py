"""Calibrate SNR -> success curves per level from simulated frames with random interference.

Fits a logistic success model p(ok | snr, level) = 1 / (1 + exp(-(snr - mid_L) / scale_L)) and stores
mid/scale per level in data/snr_model.json. Controllers use it for SNR thresholds and priors.
"""
import json
import os
import time

import numpy as np

from . import frame as fr
from .channel import AcousticChannel, Interferer
from .link import build_frame, receive_frame
from .phy import PhyConfig, MAX_LEVEL

OUT = os.path.join(os.path.dirname(__file__), "data", "snr_model.json")


def collect(cfg, frames_per_level=10, seed=0):
    rng = np.random.default_rng(seed)
    src = lambda n: rng.bytes(n)
    rows = []
    for name in ("dell-g5_to_e540", "e540_to_dell-g5"):
        ch = AcousticChannel(name, seed=seed)
        for lev in range(1, MAX_LEVEL + 1):
            for i in range(frames_per_level):
                lo = rng.uniform(800, 8000); hi = lo + rng.uniform(500, 4000)
                its = [Interferer(lo, hi, rng.uniform(-10, 15))] if i % 3 else []
                levels = np.full(cfg.n_tiles, lev)
                wave, pl = build_frame(cfg, fr.Header(seq=0, levels=levels), levels, src)
                r = receive_frame(cfg, ch(wave, interferers=its), levels=levels, sent_payloads=pl)
                rows += [(s, lev, o) for s, o in zip(r.tile_snr_db, r.tile_ok) if np.isfinite(s)]
    return np.array(rows, dtype=float)


def fit(rows):
    model = {}
    for lev in range(1, MAX_LEVEL + 1):
        d = rows[rows[:, 1] == lev]
        s, y = d[:, 0], d[:, 2]
        # 3-parameter MLE by grid search; the lapse rate absorbs rare failures SNR cannot explain
        # (mis-locked sync under heavy interference) so mid/scale describe typical behaviour
        best = (np.inf, 0, 1, 0)
        for mid in np.arange(0, 35, 0.2):
            for sc in np.arange(0.3, 5, 0.1):
                q0 = 1 / (1 + np.exp(-np.clip((s - mid) / sc, -30, 30)))
                for lapse in (0.0, 0.01, 0.02, 0.05, 0.1):
                    q = (1 - lapse) * q0
                    nll = -np.sum(y * np.log(q + 1e-9) + (1 - y) * np.log(1 - q + 1e-9))
                    if nll < best[0]:
                        best = (nll, mid, sc, lapse)
        _, mid, sc, lapse = best
        model[lev] = {"mid": float(mid), "scale": float(sc), "lapse": float(lapse),
                      "thr90": float(mid + sc * np.log(9)), "n": int(len(d))}
    return model


def load():
    with open(OUT) as f:
        return {int(k): v for k, v in json.load(f).items()}


def main():
    cfg = PhyConfig()
    t = time.time()
    rows = collect(cfg)
    model = fit(rows)
    with open(OUT, "w") as f:
        json.dump(model, f, indent=1)
    hi = rows[rows[:, 0] > 20]
    print("tiles with SNR > 20 dB that failed, per level:",
          {lev: int(((hi[:, 1] == lev) & (hi[:, 2] == 0)).sum()) for lev in range(1, MAX_LEVEL + 1)})
    for lev, m in model.items():
        print(f"L{lev}: 50% at {m['mid']:.1f} dB, 90% at {m['thr90']:.1f} dB (scale {m['scale']:.2f}, lapse {m['lapse']:.2f}, n={m['n']})")
    print(f"{len(rows)} tile samples in {time.time() - t:.0f}s")


if __name__ == "__main__":
    main()
