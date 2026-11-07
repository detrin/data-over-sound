"""An adaptive OFDM link endpoint: header feedback, Thompson-sampling adaptation, fallback.

Each node transmits in turn. The receiver of a direction owns the adaptation for it: it measures
every tile (header SNR + block outcomes), runs the controller (Thompson sampling with an SNR prior,
the winner of ofdm.simulate / ofdm.air_compare) and sends back, in its own frame header:
  fb_seq    - the peer frame it is reporting on
  fb_ok     - per-tile block success of that frame (ACK bitmap)
  fb_levels - the tile map the peer should use next
A node only learns this by decoding the peer's header; there is no side channel. Without feedback
for FALLBACK_AFTER frames, a sender falls back to the robust all-L1 map.

Traffic is pluggable: any object with fill(seq, caps) -> [bytes per tile], ack(seq, ok) and
requeue_before(seq); received blocks go to `deliver(block)`.
"""
from typing import Callable

import numpy as np

from . import frame as fr
from .controllers import ThompsonSW
from .link import RxResult, build_frame, receive_frame
from .phy import PhyConfig

FALLBACK_AFTER = 2
NO_FEEDBACK = 0xFFFF


class Node:
    def __init__(self, cfg: PhyConfig, name: str, traffic, deliver: Callable[[bytes], None], seed: int = 0):
        self.cfg, self.name = cfg, name
        self.traffic, self.deliver = traffic, deliver
        self.ctrl = ThompsonSW(cfg, gamma=0.7, snr_prior=True, seed=seed)   # for the incoming direction
        self.out_levels = np.full(cfg.n_tiles, 1)                          # robust until told otherwise
        self.told_levels = np.full(cfg.n_tiles, 1)                         # what we asked the peer to use
        self.fb = (NO_FEEDBACK, np.zeros(cfg.n_tiles, bool), self.told_levels)
        self.seq, self.missed = 0, 0
        self.stats = {"frames": 0, "lost_rx": 0, "payload_bits": 0}

    def make_frame(self) -> np.ndarray:
        cfg, seq = self.cfg, self.seq
        caps = [fr.block_layout(cfg, int(l))[1] for l in self.out_levels]
        blocks = iter(self.traffic.fill(seq, caps))
        h = fr.Header(seq=seq, levels=self.out_levels, fb_seq=self.fb[0], fb_ok=self.fb[1], fb_levels=self.fb[2])
        wave, _ = build_frame(cfg, h, self.out_levels, lambda n: next(blocks))
        self.seq = (self.seq + 1) % NO_FEEDBACK
        self.stats["frames"] += 1
        return wave

    def on_receive(self, rec: np.ndarray) -> RxResult:
        cfg = self.cfg
        r = receive_frame(cfg, rec)
        if r.header is None:                     # nothing decodable: no data, no feedback
            self.ctrl.observe(self.told_levels, np.zeros(cfg.n_tiles, bool), r.tile_snr_db)
            self._missed_feedback(lost=True)
            return r
        h = r.header
        for k in np.flatnonzero(r.tile_ok):
            self.deliver(r.blocks[k])
            self.stats["payload_bits"] += 8 * len(r.blocks[k])
        self.ctrl.observe(h.levels, r.tile_ok, r.tile_snr_db)
        if h.fb_seq != NO_FEEDBACK:              # feedback about our own last frame
            self.traffic.ack(h.fb_seq, h.fb_ok)
            self.traffic.requeue_before(h.fb_seq)
            self.out_levels, self.missed = h.fb_levels, 0
        else:
            self._missed_feedback(lost=False)
        self.told_levels = self.ctrl.choose()
        self.fb = (h.seq, r.tile_ok, self.told_levels)
        return r

    def on_lost(self):
        """No peer frame was heard at all (timeout)."""
        self._missed_feedback(lost=True)

    def _missed_feedback(self, lost: bool):
        self.stats["lost_rx"] += lost
        self.missed += 1
        if self.missed >= FALLBACK_AFTER:
            self.out_levels = np.full(self.cfg.n_tiles, 1)
            self.traffic.requeue_before(self.seq)
