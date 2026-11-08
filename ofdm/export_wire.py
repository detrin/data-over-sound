"""Export the OFDM wire constants (numpy-PRNG tables) for other implementations.

ofdm/data/wire_v1.bin, little endian:
  b"DOS1", u32 n_bins, u32 n_data, u32 n_slots,
  u8[n_bins]           reference phase indices (phase = pi/2 * k + pi/4)
  u8[n_data * n_bins]  pilot DQPSK step indices, symbol-major
  u16[n_slots]         header byte-slot permutation (np.random.default_rng(7).permutation)
"""
import os
import struct

import numpy as np

from .phy import PhyConfig, _rng_phases

OUT = os.path.join(os.path.dirname(__file__), "data", "wire_v1.bin")


def main(cfg=PhyConfig()):
    ref = _rng_phases(cfg, 0, len(cfg.bins)).astype(np.uint8)
    pil = _rng_phases(cfg, 1, (cfg.n_data, len(cfg.bins))).astype(np.uint8)
    slots = np.random.default_rng(7).permutation(cfg.header_bits // 8).astype("<u2")
    with open(OUT, "wb") as f:
        f.write(b"DOS1" + struct.pack("<III", len(cfg.bins), cfg.n_data, len(slots)))
        f.write(ref.tobytes()); f.write(pil.tobytes()); f.write(slots.tobytes())
    print(f"wrote {OUT}: {len(cfg.bins)} bins, {cfg.n_data} data symbols, {len(slots)} header slots")


if __name__ == "__main__":
    main()
