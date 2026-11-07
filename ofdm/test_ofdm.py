"""Offline regression tests: python -m ofdm.test_ofdm"""
import numpy as np

from . import frame as fr
from .packets import PacketQueue, PacketReassembly
from .stream import Reassembly, Sender
from .link import build_frame, receive_frame
from .phy import PhyConfig


def test_loopback_all_levels(cfg=PhyConfig()):
    rng = np.random.default_rng(0)
    levels = np.array([l % 4 for l in range(cfg.n_tiles)])
    wave, pl = build_frame(cfg, fr.Header(seq=7, levels=levels), levels, lambda n: rng.bytes(n))
    r = receive_frame(cfg, np.concatenate([np.zeros(3000), wave, np.zeros(3000)]), sent_payloads=pl)
    assert r.header is not None and r.header.seq == 7
    assert (r.header.levels == levels).all()
    assert r.tile_ok[levels > 0].all()


def test_header_roundtrip(cfg=PhyConfig()):
    rng = np.random.default_rng(1)
    h = fr.Header(seq=123, levels=rng.integers(0, 4, cfg.n_tiles), fb_seq=122,
                  fb_ok=rng.random(cfg.n_tiles) < 0.5, fb_levels=rng.integers(0, 4, cfg.n_tiles))
    h2 = fr.Header.unpack(h.pack(cfg.n_tiles), cfg.n_tiles)
    assert (h2.seq, h2.fb_seq) == (123, 122)
    assert (h2.levels == h.levels).all() and (h2.fb_levels == h.fb_levels).all() and (h2.fb_ok == h.fb_ok).all()


def test_selective_repeat_over_lossy_link():
    rng = np.random.default_rng(2)
    data = rng.bytes(5000)
    tx, rx = Sender(data, seed=3), Reassembly()
    caps = [10, 70, 106, 0] * 10
    for seq in range(200):
        blocks = tx.fill(seq, caps)
        ok = rng.random(len(caps)) > 0.4
        ok[0] = False                                  # tile 0 always fails: must not starve anything
        for k, b in enumerate(blocks):
            if ok[k] and b:
                rx.put(b)
        if rng.random() < 0.8:                         # feedback sometimes lost
            tx.ack(seq, ok)
            tx.requeue_before(seq)
        if tx.done:
            break
    assert tx.done and rx.done and rx.data == data


def test_ip_packets_fragment_retransmit_reassemble():
    rng = np.random.default_rng(4)
    pkts = [rng.bytes(int(n)) for n in rng.integers(20, 1200, 40)]
    got = []
    q, rx = PacketQueue(seed=5), PacketReassembly(got.append)
    for p in pkts:
        q.push(p)
    q.push(pkts[0])                                    # identical packet still waiting: dropped
    caps = [10, 70, 106, 0] * 23
    for seq in range(60):
        blocks = q.fill(seq, caps)
        ok = rng.random(len(caps)) > 0.2
        for k, b in enumerate(blocks):
            if ok[k] and b:
                rx.put(b)
        q.ack(seq, ok)
    assert q.stats["dropped_dup"] == 1
    # contract: delivered packets are intact and unique; a packet may only be missing if one of its
    # fragments was given up after MAX_TRIES (the transport, e.g. TCP, recovers those)
    assert all(g in pkts for g in got) and len(set(got)) == len(got)
    missing = len(pkts) - len(got)
    assert missing <= q.stats["given_up"], (missing, q.stats)
    assert len(got) >= 0.85 * len(pkts)


if __name__ == "__main__":
    for name, fn in list(globals().items()):
        if name.startswith("test_"):
            fn()
            print("ok", name)
