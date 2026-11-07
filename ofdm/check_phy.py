"""Sanity check: loopback and simulated-channel decoding at fixed levels."""
import numpy as np

from . import frame as fr
from .channel import AcousticChannel
from .link import build_frame, receive_frame
from .phy import PhyConfig


def main(cfg=PhyConfig()):
    rng = np.random.default_rng(0)
    src = lambda n: rng.bytes(n)
    print(f"tiles={cfg.n_tiles} bins={len(cfg.bins)} frame={cfg.frame_s:.2f}s header_bits={cfg.header_bits}")
    chans = [("loopback", None)] + [(n, AcousticChannel(n, seed=1)) for n in ("dell-g5_to_e540", "e540_to_dell-g5")]
    for name, ch in chans:
        for lev in (1, 2, 3):
            levels = np.full(cfg.n_tiles, lev)
            wave, pl = build_frame(cfg, fr.Header(seq=lev, levels=levels), levels, src)
            rx = np.concatenate([np.zeros(5000), wave, np.zeros(5000)]) if ch is None else ch(wave)
            r = receive_frame(cfg, rx, sent_payloads=pl)
            print(f"{name:16s} L{lev}: hdr {'ok ' if r.header else 'BAD'} ok {r.tile_ok.sum():2d}/{cfg.n_tiles} "
                  f"{8 * r.payload_bytes / cfg.frame_s:5.0f} bit/s drift {r.drift_ppm:5.1f}ppm  snr:",
                  " ".join(f"{s:.0f}" for s in r.tile_snr_db))


if __name__ == "__main__":
    main()
