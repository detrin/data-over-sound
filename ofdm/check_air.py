"""Over-the-air check: fixed-level frames both directions, compared with the simulator."""
from dataclasses import replace
import numpy as np

from . import air, frame as fr
from .channel import AcousticChannel
from .link import build_frame, receive_frame
from .phy import PhyConfig

BASE = PhyConfig()
CONFIGS = {"640/160 10k": BASE}


def main():
    rng = np.random.default_rng(0)
    src = lambda n: rng.bytes(n)
    for tx in air.HOSTS:
        name = f"{tx}_to_{air.peer(tx)}"
        for label, cfg in CONFIGS.items():
            for lev in (2, 3):
                levels = np.full(cfg.n_tiles, lev)
                wave, pl = build_frame(cfg, fr.Header(seq=lev, levels=levels), levels, src)
                for where, rx in (("air", air.transmit(tx, wave)), ("sim", AcousticChannel(name, seed=2)(wave))):
                    r = receive_frame(cfg, rx, levels=levels, sent_payloads=pl)
                    print(f"{name} {label} L{lev} {where}: hdr {'ok ' if r.header else 'BAD'} "
                          f"ok {r.tile_ok.sum():2d}/{cfg.n_tiles} {8 * r.payload_bytes / cfg.frame_s:5.0f} bit/s "
                          f"drift {r.drift_ppm:6.1f}ppm rx peak {np.abs(rx).max():.2f} "
                          f"snr med {np.median(r.tile_snr_db):4.1f}")


if __name__ == "__main__":
    main()
