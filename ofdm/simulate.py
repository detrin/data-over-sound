"""Compare link-adaptation strategies in the calibrated channel simulator under changing noise."""
import argparse
import time
from collections import defaultdict

import numpy as np

from . import frame as fr
from .channel import AcousticChannel
from .controllers import all_strategies, finalists
from .link import build_frame, receive_frame
from .phy import PhyConfig
from .scenario import PHASES, schedule


def run(ctrl, cfg, channel_name, frames_per_phase, seed):
    ch = AcousticChannel(channel_name, seed=seed)
    rng = np.random.default_rng(seed)
    src = lambda n: rng.bytes(n)
    per_phase = defaultdict(float)
    lost = 0
    for seq, (pi, _, its) in enumerate(schedule(frames_per_phase)):
        levels = ctrl.choose()
        wave, pl = build_frame(cfg, fr.Header(seq=seq, levels=levels), levels, src)
        r = receive_frame(cfg, ch(wave, interferers=its), sent_payloads=pl)   # levels from the header
        lost += r.header is None
        ctrl.observe(levels, r.tile_ok, r.tile_snr_db)
        per_phase[pi] += 8 * r.payload_bytes
    airtime = frames_per_phase * cfg.frame_s
    return np.array([per_phase[i] / airtime for i in range(len(PHASES))]), lost


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--frames-per-phase", type=int, default=8)
    ap.add_argument("--seeds", type=int, default=2)
    ap.add_argument("--finalists", action="store_true", help="only the round-2 strategies")
    ap.add_argument("--snr-bias-db", type=float, default=0.0,
                    help="shift the controllers' SNR model by this much (simulates miscalibration)")
    args = ap.parse_args()
    cfg = PhyConfig()
    t = time.time()
    results = defaultdict(list)
    for name in ("dell-g5_to_e540", "e540_to_dell-g5"):
        for seed in range(args.seeds):
            for ctrl in (finalists if args.finalists else all_strategies)(cfg, seed=seed):
                if hasattr(ctrl, "model"):
                    for m in ctrl.model.values():
                        m["mid"] += args.snr_bias_db
                rates, lost = run(ctrl, cfg, name, args.frames_per_phase, seed)
                results[ctrl.name].append((rates, lost))
    print(f"goodput bit/s per phase (mean over 2 directions x {args.seeds} seeds), {time.time() - t:.0f}s")
    print(f"{'strategy':20s}" + "".join(f"{n[:14]:>15s}" for n, _ in PHASES) + f"{'MEAN':>9s}{'lost':>6s}")
    rows = []
    for name, rs in results.items():
        rates = np.mean([r for r, _ in rs], axis=0)
        rows.append((rates.mean(), name, rates, np.mean([l for _, l in rs])))
    for mean, name, rates, lost in sorted(rows, reverse=True):
        print(f"{name:20s}" + "".join(f"{v:15.0f}" for v in rates) + f"{mean:9.0f}{lost:6.1f}")


if __name__ == "__main__":
    main()
