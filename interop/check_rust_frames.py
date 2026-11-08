"""Decode frames written by the Rust implementation (`dos interop-gen DIR`) with the Python one."""
import json
import os
import sys

import numpy as np
from scipy.io import wavfile

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))
from ofdm.link import receive_frame  # noqa: E402
from ofdm.phy import PhyConfig  # noqa: E402

cfg, d, failures = PhyConfig(), sys.argv[1], 0
for name in ("mixed", "l3", "l1"):
    meta = json.load(open(f"{d}/frame_{name}.json"))
    fs, x = wavfile.read(f"{d}/frame_{name}.wav")
    r = receive_frame(cfg, x.astype(float) / 32768)
    h = r.header
    ok = (h is not None and h.seq == meta["seq"] and h.fb_seq == meta["fb_seq"]
          and list(h.levels) == meta["levels"] and list(h.fb_levels) == meta["fb_levels"]
          and list(h.fb_ok.astype(int)) == meta["fb_ok"])
    blocks_ok = all(lev == 0 or (r.blocks[k] is not None and r.blocks[k].hex() == meta["payloads"][k])
                    for k, lev in enumerate(meta["levels"]))
    print(f"python decodes rust frame_{name}: header {'OK' if ok else 'MISMATCH'}, "
          f"payloads {'OK' if blocks_ok else 'MISMATCH'} ({int(r.tile_ok.sum())}/{cfg.n_tiles} tiles)")
    failures += not (ok and blocks_ok)
sys.exit(failures)
