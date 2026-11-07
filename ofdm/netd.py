"""ofdmnet daemon: the adaptive OFDM modem as a network card (TUN device, e.g. ofdm0).

Run one per machine inside the desktop session (PipeWire), after `ofdmnet.sh up` created the
user-owned TUN device:

    python3 -m ofdm.netd --role leader      # on one machine
    python3 -m ofdm.netd --role follower    # on the other

Medium access is strict turn-taking (no collisions): the leader sends a frame and listens; the
follower answers every frame it hears. If the leader hears nothing within a timeout it sends
again. When neither side has traffic the leader slows down to one exchange per --idle-gap seconds
(new outgoing packets wake it up immediately). Each node ignores audio recorded while it was
transmitting, plus a reverb tail.
"""
import argparse
import threading
import time

import numpy as np

from .audio import FS, Recorder, play
from .node import Node
from .packets import PacketQueue, PacketReassembly
from .phy import PhyConfig, find_sync
from .tun import Tun

TAIL_S = 0.7            # own reverb tail to skip after transmitting
DECODE_MARGIN_S = 0.4   # audio to wait for after the frame body before decoding
FALSE_ALARM_SNR_DB = 3.0  # header lost and SNR this low: it was noise, not a peer frame


class Modem:
    def __init__(self, cfg: PhyConfig, dev: str, name: str, log):
        self.cfg, self.log = cfg, log
        self.rec = Recorder()
        self.tun = Tun(dev)
        self.queue = PacketQueue()
        self.reasm = PacketReassembly(self.tun.write)
        self.node = Node(cfg, name, self.queue, self.reasm.put)
        self.wake = threading.Event()
        self.body = len(cfg.sync) + int(cfg.gap_s * cfg.fs) + cfg.n_syms * cfg.sym_len
        threading.Thread(target=self._tun_reader, daemon=True).start()

    def _tun_reader(self):
        while True:
            self.queue.push(self.tun.read())
            self.wake.set()

    def transmit(self) -> int:
        """Send one frame; return the absolute sample index to start listening from."""
        wave = self.node.make_frame()
        play(wave)
        return self.rec.now + int(TAIL_S * FS)

    def wait_frame(self, listen_from: int, timeout_s: float):
        """(segment containing one peer frame, new listen index) or (None, listen index)."""
        deadline = time.time() + timeout_s
        while time.time() < deadline:
            time.sleep(0.4)
            x = self.rec.since(listen_from)
            if len(x) > 3 * self.body:                 # nothing found in a long stretch: slide on
                listen_from += len(x) - 2 * self.body
                x = self.rec.since(listen_from)
            if len(x) < self.body // 2:
                continue
            st = find_sync(self.cfg, x)                # first sample after the chirp
            if st < 0:
                continue
            end = st - len(self.cfg.sync) + self.body + int(DECODE_MARGIN_S * FS)
            if len(x) < end:
                continue                               # frame still arriving
            begin = max(0, st - len(self.cfg.sync) - FS // 10)
            if self.is_false_alarm(x[begin:end]):
                self.log("ignored a sync hit on noise")
                listen_from += st
                continue
            return x[begin:end], listen_from + end
        return None, listen_from

    def is_false_alarm(self, seg) -> bool:
        """A sync hit whose header fails with noise-level SNR is not a frame: keep listening."""
        from .link import receive_frame
        r = receive_frame(self.cfg, seg)
        return r.header is None and not (np.nanmedian(r.tile_snr_db) > FALSE_ALARM_SNR_DB)

    def receive(self, seg) -> None:
        t = time.time()
        r = self.node.on_receive(seg)
        lv = np.bincount(np.asarray(r.header.levels if r.header else [], int), minlength=4)
        self.log(f"rx seq {r.header.seq if r.header else '----':>5} "
                 f"{'hdr ok ' if r.header else 'HDR LOST'} tiles ok {r.tile_ok.sum():2d}/{self.cfg.n_tiles} "
                 f"levels p/1/2/3 {lv[0]}/{lv[1]}/{lv[2]}/{lv[3]} payload {r.payload_bytes:5d} B "
                 f"snr med {np.nanmedian(r.tile_snr_db):4.1f} dB | tx queue {self.queue.pending_bytes} B "
                 f"| delivered {self.reasm.stats['delivered']} pkts | decode {time.time() - t:.1f}s")
        return r


def run_leader(m: Modem, idle_gap: float):
    timeout = m.cfg.frame_s + 8.0
    while True:
        listen_from = m.transmit()
        seg, _ = m.wait_frame(listen_from, timeout)
        if seg is None:
            m.node.on_lost()
            m.log("no answer from follower (timeout)")
            continue
        r = m.receive(seg)
        idle = m.queue.pending_bytes == 0 and (r.header is None or r.payload_bytes == 0)
        if idle:
            m.wake.clear()
            m.wake.wait(idle_gap)


def run_follower(m: Modem):
    listen_from = m.rec.now
    while True:
        seg, listen_from = m.wait_frame(listen_from, 3600)
        if seg is None:
            continue
        m.receive(seg)
        listen_from = m.transmit()


def main():
    ap = argparse.ArgumentParser(description="Adaptive OFDM acoustic modem as a network card")
    ap.add_argument("--role", choices=["leader", "follower"], required=True)
    ap.add_argument("--dev", default="ofdm0")
    ap.add_argument("--idle-gap", type=float, default=4.0, help="leader pause between idle exchanges (s)")
    args = ap.parse_args()
    cfg = PhyConfig()
    log = lambda msg: print(f"{time.strftime('%H:%M:%S')} [{args.role}] {msg}", flush=True)
    log(f"{cfg.n_tiles} tiles {cfg.f_lo:.0f}-{cfg.f_hi:.0f} Hz, frame {cfg.frame_s:.1f}s on {args.dev}")
    m = Modem(cfg, args.dev, args.role, log)
    (run_leader(m, args.idle_gap) if args.role == "leader" else run_follower(m))


if __name__ == "__main__":
    main()
