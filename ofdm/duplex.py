"""Two-way adaptive acoustic link: TDD, in-band feedback, selective-repeat ARQ.

Each node transmits in turn. The receiver of a direction owns the adaptation for it: it measures
every tile (header SNR + block outcomes), runs the controller (Thompson sampling with an SNR prior,
the winner of ofdm.simulate / ofdm.air_compare) and sends back, in its own frame header:
  fb_seq    - the peer frame it is reporting on
  fb_ok     - per-tile block success of that frame (ACK bitmap)
  fb_levels - the tile map the peer should use next
A node only learns this by decoding the peer's header; there is no side channel. Without feedback
for FALLBACK_AFTER frames, a sender falls back to the robust all-L1 map.

Tile payload: [offset 3 B][length 2 B][total length 3 B][data][zero padding]; failed or
unacknowledged ranges are resent.
"""
import argparse
import hashlib
import time
from typing import Callable, List, Optional

import numpy as np

from . import frame as fr
from .channel import AcousticChannel
from .controllers import ThompsonSW
from .link import build_frame, receive_frame
from .phy import PhyConfig

BLOCK_HDR = 8
FALLBACK_AFTER = 2


class Sender:
    """Outgoing byte stream with selective repeat: bytes are needed, in flight or delivered."""

    def __init__(self, data: bytes, seed: int = 0):
        self.data = data
        self.rng = np.random.default_rng(seed)
        self.needed = np.ones(len(data), bool)
        self.delivered = np.zeros(len(data), bool)
        self.inflight = {}                     # seq -> [(tile, offset, length)]

    @property
    def done(self) -> bool:
        return bool(self.delivered.all())

    def fill(self, seq: int, caps: List[int]) -> List[bytes]:
        """Payload for each tile (cap = payload bytes, 0 for pilots). The oldest needed bytes (often
        retransmissions) go to the largest-capacity, i.e. most reliable, tiles first, so a bad tile
        cannot starve a range by failing it over and over."""
        out, sent = [b""] * len(caps), []
        order = sorted(range(len(caps)), key=lambda t: (-caps[t], self.rng.random()))
        for tile in order:
            cap = caps[tile]
            room = cap - BLOCK_HDR
            free = np.flatnonzero(self.needed)
            if room <= 0 or not len(free):
                continue
            off = int(free[0])
            end = off + 1
            while end < len(self.data) and end - off < room and self.needed[end]:
                end += 1
            self.needed[off:end] = False
            sent.append((tile, off, end - off))
            out[tile] = (off.to_bytes(3, "big") + (end - off).to_bytes(2, "big")
                         + len(self.data).to_bytes(3, "big") + self.data[off:end])
        self.inflight[seq] = sent
        return out

    def ack(self, seq: int, ok: np.ndarray):
        for tile, off, n in self.inflight.pop(seq, []):
            if ok[tile]:
                self.delivered[off:off + n] = True
            else:
                self.needed[off:off + n] = ~self.delivered[off:off + n]

    def requeue_before(self, seq: int):
        """Frames older than `seq` that never got feedback: their bytes are needed again."""
        for s in [s for s in self.inflight if s < seq]:
            self.ack(s, np.zeros(10_000, bool))


class Reassembly:
    def __init__(self):
        self.buf, self.have, self.total = None, None, None

    def put(self, block: bytes):
        if len(block) < BLOCK_HDR:
            return
        off, n = int.from_bytes(block[:3], "big"), int.from_bytes(block[3:5], "big")
        total = int.from_bytes(block[5:8], "big")
        if self.buf is None:
            self.buf, self.have, self.total = bytearray(total), np.zeros(total, bool), total
        data = block[BLOCK_HDR:BLOCK_HDR + n][: max(0, self.total - off)]
        self.buf[off:off + len(data)] = data
        self.have[off:off + len(data)] = True

    @property
    def done(self) -> bool:
        return self.have is not None and bool(self.have.all())

    @property
    def data(self) -> bytes:
        return bytes(self.buf or b"")


class Node:
    def __init__(self, cfg: PhyConfig, name: str, data: bytes, seed: int = 0):
        self.cfg, self.name = cfg, name
        self.tx, self.rx = Sender(data, seed), Reassembly()
        self.ctrl = ThompsonSW(cfg, gamma=0.7, snr_prior=True, seed=seed)   # for the incoming direction
        self.out_levels = np.full(cfg.n_tiles, 1)                          # robust until told otherwise
        self.told_levels = np.full(cfg.n_tiles, 1)                         # what we asked the peer to use
        self.fb = (0xFFFF, np.zeros(cfg.n_tiles, bool), self.told_levels)
        self.seq, self.missed = 0, 0
        self.stats = {"frames": 0, "lost_rx": 0, "payload_bits": 0}

    def make_frame(self) -> np.ndarray:
        cfg, seq = self.cfg, self.seq
        caps = [fr.block_layout(cfg, int(l))[1] for l in self.out_levels]
        blocks = iter(self.tx.fill(seq, caps))
        h = fr.Header(seq=seq, levels=self.out_levels, fb_seq=self.fb[0], fb_ok=self.fb[1], fb_levels=self.fb[2])
        wave, _ = build_frame(cfg, h, self.out_levels, lambda n: next(blocks))
        self.seq = (self.seq + 1) % 0xFFFF
        self.stats["frames"] += 1
        return wave

    def on_receive(self, rec: np.ndarray):
        cfg = self.cfg
        r = receive_frame(cfg, rec)
        if r.header is None:                     # nothing decodable: no data, no feedback
            self.stats["lost_rx"] += 1
            self.ctrl.observe(self.told_levels, np.zeros(cfg.n_tiles, bool), r.tile_snr_db)
            self.missed += 1
        else:
            h = r.header
            # data: decode again with the payloads to deliver them (receive_frame verified CRCs)
            for k in np.flatnonzero(r.tile_ok):
                self.rx.put(r.blocks[k])
                self.stats["payload_bits"] += 8 * len(r.blocks[k])
            self.ctrl.observe(h.levels, r.tile_ok, r.tile_snr_db)
            # feedback about our own last frame
            if h.fb_seq != 0xFFFF:
                self.tx.ack(h.fb_seq, h.fb_ok)
                self.tx.requeue_before(h.fb_seq)
                self.out_levels, self.missed = h.fb_levels, 0
            else:
                self.missed += 1
            self.told_levels = self.ctrl.choose()
            self.fb = (h.seq, r.tile_ok, self.told_levels)
        if self.missed >= FALLBACK_AFTER:
            self.out_levels = np.full(cfg.n_tiles, 1)
            self.tx.requeue_before(self.seq)


def run(cfg: PhyConfig, data_a: bytes, data_b: bytes,
        link_ab: Callable, link_ba: Callable, max_rounds: int = 40, log=print):
    """link_xy(wave, round) -> recording at y. Runs until both transfers are delivered and acknowledged."""
    a, b = Node(cfg, "A", data_a, seed=1), Node(cfg, "B", data_b, seed=2)
    t0 = time.time()
    for rnd in range(max_rounds):
        b.on_receive(link_ab(a.make_frame(), rnd))
        a.on_receive(link_ba(b.make_frame(), rnd))
        log(f"round {rnd:2d}: A->B {b.rx.have.mean() if b.rx.have is not None else 0:6.1%} "
            f"(A map {''.join(map(str, a.out_levels))})  B->A {a.rx.have.mean() if a.rx.have is not None else 0:6.1%} "
            f"(B map {''.join(map(str, b.out_levels))})")
        if a.tx.done and b.tx.done:
            break
    airtime = (a.stats["frames"] + b.stats["frames"]) * cfg.frame_s
    return a, b, rnd + 1, airtime, time.time() - t0


def main():
    ap = argparse.ArgumentParser(description="Two-way adaptive OFDM transfer (sim or over the air)")
    ap.add_argument("--air", action="store_true", help="use the real laptops (default: simulator)")
    ap.add_argument("--file-a", default="Feels_good_man.jpg", help="sent dell-g5 -> e540")
    ap.add_argument("--file-b", default="input.txt", help="sent e540 -> dell-g5")
    ap.add_argument("--noise", action="store_true", help="broadcast changing noise (scenario phases)")
    ap.add_argument("--rounds-per-phase", type=int, default=2)
    args = ap.parse_args()
    cfg = PhyConfig()
    data_a, data_b = open(args.file_a, "rb").read(), open(args.file_b, "rb").read()
    from .scenario import PHASES
    def phase(rnd):
        return PHASES[min(rnd // args.rounds_per_phase, len(PHASES) - 1)] if args.noise else ("quiet", [])

    if args.air:
        from . import air
        from .air_compare import calibrate
        rng = np.random.default_rng(0)
        cal = {tx: calibrate(tx, cfg, rng) for tx in air.HOSTS} if args.noise else {}
        noise_len = int((cfg.frame_s + 3) * cfg.fs)
        def link(tx):
            def go(wave, rnd):
                _, its = phase(rnd)
                noise = None
                if its:
                    sig, gain = cal[tx]
                    noise = sum(it.render(noise_len, cfg.fs, sig / gain, rng) for it in its)
                return air.transmit(tx, wave, interferer=noise)
            return go
        link_ab, link_ba = link("dell-g5"), link("e540")
    else:
        chans = {"ab": AcousticChannel("dell-g5_to_e540", seed=1), "ba": AcousticChannel("e540_to_dell-g5", seed=2)}
        link_ab = lambda w, rnd: chans["ab"](w, interferers=phase(rnd)[1])
        link_ba = lambda w, rnd: chans["ba"](w, interferers=phase(rnd)[1])

    a, b, rounds, airtime, wall = run(cfg, data_a, data_b, link_ab, link_ba,
                                      log=lambda m: print(m, flush=True))
    for label, sent, got in (("A->B", data_a, b.rx.data), ("B->A", data_b, a.rx.data)):
        ok = sent == got
        print(f"{label}: {len(sent)} bytes, {'IDENTICAL' if ok else 'MISMATCH'} "
              f"sha256 {hashlib.sha256(got).hexdigest()[:16]}")
    for node in (a, b):
        if not node.tx.done:
            print(f"{node.name}: {int((~node.tx.delivered).sum())} bytes undelivered, "
                  f"{int(node.tx.needed.sum())} queued, {len(node.tx.inflight)} frames in flight")
    total = len(data_a) + len(data_b)
    print(f"{rounds} rounds, {airtime:.0f}s airtime ({wall:.0f}s wall): "
          f"{8 * total / airtime:.0f} bit/s goodput, both directions combined")


if __name__ == "__main__":
    main()
