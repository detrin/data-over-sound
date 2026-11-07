"""File (byte stream) traffic for the adaptive link: selective repeat over tiles.

Tile payload: [offset 3 B][length 2 B][total length 3 B][data][zero padding]; failed or
unacknowledged ranges are resent until everything is delivered.
"""
from typing import List

import numpy as np

BLOCK_HDR = 8


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
