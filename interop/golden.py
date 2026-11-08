"""Golden vectors from the Python reference implementation for the Rust port.

interop/golden/rs.json        Reed-Solomon (reedsolo) encodings + corrupted codewords
interop/golden/header.json    Header.pack() outputs
interop/golden/frame_*.wav    full frames (16-bit), with frame_*.json: levels, header, payloads
interop/golden/sync.json      chirp samples (first/last) and bins for numerical checks
"""
import json
import os
import sys

import numpy as np
import reedsolo
from scipy.io import wavfile

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))
sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))
from ofdm import frame as fr  # noqa: E402
from ofdm.link import build_frame  # noqa: E402
from ofdm.phy import PhyConfig  # noqa: E402

OUT = os.path.join(os.path.dirname(__file__), "golden")


def main():
    os.makedirs(OUT, exist_ok=True)
    rng = np.random.default_rng(42)
    cfg = PhyConfig()
    rs = []
    for n, nsym in ((10, 4), (48, 12), (100, 24), (200, 50), (64, 191)):
        msg = rng.bytes(n)
        cw = bytes(reedsolo.RSCodec(nsym).encode(msg))
        bad = bytearray(cw)
        for i in rng.choice(len(cw), nsym // 2, replace=False):
            bad[i] ^= int(rng.integers(1, 256))
        rs.append({"nsym": nsym, "msg": msg.hex(), "cw": cw.hex(), "corrupted": bytes(bad).hex()})
    json.dump(rs, open(f"{OUT}/rs.json", "w"), indent=1)

    hdrs = []
    for _ in range(5):
        h = fr.Header(seq=int(rng.integers(0, 65535)), levels=rng.integers(0, 4, cfg.n_tiles),
                      fb_seq=int(rng.integers(0, 65536)), fb_ok=rng.random(cfg.n_tiles) < 0.5,
                      fb_levels=rng.integers(0, 4, cfg.n_tiles))
        hdrs.append({"seq": h.seq, "fb_seq": h.fb_seq, "levels": h.levels.tolist(), "fb_levels": h.fb_levels.tolist(),
                     "fb_ok": h.fb_ok.astype(int).tolist(), "packed": h.pack(cfg.n_tiles).hex()})
    json.dump(hdrs, open(f"{OUT}/header.json", "w"), indent=1)

    for name, levels in (("mixed", np.arange(cfg.n_tiles) % 4), ("l3", np.full(cfg.n_tiles, 3)),
                         ("l1", np.full(cfg.n_tiles, 1))):
        h = fr.Header(seq=77, levels=levels, fb_seq=76, fb_ok=np.arange(cfg.n_tiles) % 3 == 0,
                      fb_levels=(np.arange(cfg.n_tiles) + 1) % 4)
        wave, payloads = build_frame(cfg, h, levels, lambda n: rng.bytes(n))
        x = np.concatenate([np.zeros(4800), wave, np.zeros(4800)])
        wavfile.write(f"{OUT}/frame_{name}.wav", cfg.fs, (np.clip(x, -1, 1) * 32767).astype(np.int16))
        json.dump({"levels": levels.tolist(), "seq": 77, "fb_seq": 76, "payloads": [p.hex() for p in payloads],
                   "fb_ok": h.fb_ok.astype(int).tolist(), "fb_levels": h.fb_levels.tolist()},
                  open(f"{OUT}/frame_{name}.json", "w"))
    json.dump({"sync_head": cfg.sync[:8].tolist(), "sync_tail": cfg.sync[-8:].tolist(), "sync_len": len(cfg.sync),
               "n_bins": len(cfg.bins), "bin0": int(cfg.bins[0]), "n_tiles": cfg.n_tiles,
               "frame_len": int(round(cfg.frame_s * cfg.fs))}, open(f"{OUT}/sync.json", "w"), indent=1)
    # legacy minimodem framing: ARQ packet -> CRC32 -> RS(16) -> COBS -> base64 markers
    from packet_framer import ARQPacket, PacketFramer, PacketType
    from sound_packet_bridge import wrap
    framer = PacketFramer(use_fec=True, fec_nsym=16)
    legacy = []
    for kind, seq, total, payload in ((PacketType.DATA, 0, 3, rng.bytes(64)), (PacketType.DATA, 2, 3, b"\0zeros\0\0" * 3),
                                      (PacketType.ACK, 1, 3, b""), (PacketType.EOT, 3, 3, b"")):
        raw = ARQPacket(kind, seq, total, payload).to_bytes()
        legacy.append({"raw": raw.hex(), "framed": framer.encode(raw).hex(), "wrapped": wrap(framer.encode(raw)).hex()})
    json.dump(legacy, open(f"{OUT}/legacy.json", "w"), indent=1)
    print("golden vectors written to", OUT)


if __name__ == "__main__":
    main()
