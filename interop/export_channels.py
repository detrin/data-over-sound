"""Convert the measured channels (ofdm/data/channel_*.npz) to JSON for the Rust implementation."""
import glob
import json
import os

import numpy as np

for path in glob.glob(os.path.join(os.path.dirname(__file__), "..", "ofdm", "data", "channel_*.npz")):
    d = np.load(path)
    out = {k: (d[k].tolist() if d[k].ndim else float(d[k])) for k in ("ir", "gain", "noise_f", "noise_psd", "noise_rms")}
    json.dump(out, open(path.replace(".npz", ".json"), "w"))
    print("wrote", path.replace(".npz", ".json"))
