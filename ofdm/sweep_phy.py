"""Pick the PHY numerology: oracle goodput (best level per tile, known in hindsight) in the
calibrated channel simulator, for several symbol / guard / FEC / band choices."""
from dataclasses import replace
import numpy as np

from . import frame as fr
from .channel import AcousticChannel
from .link import build_frame, receive_frame
from .phy import PhyConfig, MAX_LEVEL

B = PhyConfig()
CONFIGS = {
    "160/20  rs10": B,
    "160/60  rs10": replace(B, ncp=2880),
    "320/80  rs10": replace(B, nfft=15360, ncp=3840, tile_sc=64, n_data=6),
    "320/80  rs25": replace(B, nfft=15360, ncp=3840, tile_sc=64, n_data=6, rs_fraction=0.25),
    "320/120 rs25": replace(B, nfft=15360, ncp=5760, tile_sc=64, n_data=6, rs_fraction=0.25),
    "640/160 rs25": replace(B, nfft=30720, ncp=7680, tile_sc=128, n_data=3, rs_fraction=0.25),
    "320/80 rs25 8k": replace(B, nfft=15360, ncp=3840, tile_sc=64, n_data=6, rs_fraction=0.25, f_hi=8000),
    "320/80 rs40": replace(B, nfft=15360, ncp=3840, tile_sc=64, n_data=6, rs_fraction=0.40),
}


def oracle_goodput(cfg, name, trials=3, seed=0):
    rng = np.random.default_rng(seed)
    src = lambda n: rng.bytes(n)
    ch = AcousticChannel(name, seed=seed)
    gain = np.zeros((MAX_LEVEL + 1, cfg.n_tiles))          # expected payload bytes per tile per level
    for lev in range(1, MAX_LEVEL + 1):
        levels = np.full(cfg.n_tiles, lev)
        n = fr.block_layout(cfg, lev)[1]
        for _ in range(trials):
            wave, pl = build_frame(cfg, fr.Header(seq=0, levels=levels), levels, src)
            r = receive_frame(cfg, ch(wave), levels=levels, sent_payloads=pl)
            gain[lev] += r.tile_ok * n / trials
    best = gain.max(axis=0)
    return 8 * best.sum() / cfg.frame_s, np.argmax(gain, axis=0)


def main():
    for name in ("dell-g5_to_e540", "e540_to_dell-g5"):
        print("==", name)
        for label, cfg in CONFIGS.items():
            g, lv = oracle_goodput(cfg, name)
            print(f"  {label:15s} frame {cfg.frame_s:4.1f}s  oracle goodput {g:6.0f} bit/s  levels {''.join(map(str, lv))}")


if __name__ == "__main__":
    main()
