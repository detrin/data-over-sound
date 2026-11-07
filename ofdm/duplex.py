"""Two-way adaptive file transfer over the OFDM link (simulator or the real laptops).

Both nodes alternate frames (see ofdm.node for the feedback/adaptation and ofdm.stream for the
selective-repeat byte stream). Runs until both files are delivered and acknowledged.
"""
import argparse
import hashlib
import time
from typing import Callable

import numpy as np

from .channel import AcousticChannel
from .node import Node
from .phy import PhyConfig
from .stream import Reassembly, Sender


def run(cfg: PhyConfig, data_a: bytes, data_b: bytes,
        link_ab: Callable, link_ba: Callable, max_rounds: int = 40, log=print):
    """link_xy(wave, round) -> recording at y. Runs until both transfers are delivered and acknowledged."""
    rx_a, rx_b = Reassembly(), Reassembly()
    a = Node(cfg, "A", Sender(data_a, seed=1), rx_a.put, seed=1)
    b = Node(cfg, "B", Sender(data_b, seed=2), rx_b.put, seed=2)
    a.rx, b.rx = rx_a, rx_b
    t0 = time.time()
    for rnd in range(max_rounds):
        b.on_receive(link_ab(a.make_frame(), rnd))
        a.on_receive(link_ba(b.make_frame(), rnd))
        log(f"round {rnd:2d}: A->B {b.rx.have.mean() if b.rx.have is not None else 0:6.1%} "
            f"(A map {''.join(map(str, a.out_levels))})  B->A {a.rx.have.mean() if a.rx.have is not None else 0:6.1%} "
            f"(B map {''.join(map(str, b.out_levels))})")
        if a.traffic.done and b.traffic.done:
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
        t = node.traffic
        if not t.done:
            print(f"{node.name}: {int((~t.delivered).sum())} bytes undelivered, "
                  f"{int(t.needed.sum())} queued, {len(t.inflight)} frames in flight")
    total = len(data_a) + len(data_b)
    print(f"{rounds} rounds, {airtime:.0f}s airtime ({wall:.0f}s wall): "
          f"{8 * total / airtime:.0f} bit/s goodput, both directions combined")


if __name__ == "__main__":
    main()
