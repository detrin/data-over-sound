"""IP packet traffic for the adaptive link (the network-card mode).

Packets are cut into fragments at fill time so they pack the tiles of the current map tightly.
A tile block holds one or more fragments, each [packet id 2 B][offset 2 B][length 2 B][packet
length 2 B][data]; a zero packet id ends the block (padding).

Light link-layer ARQ: a fragment in a failed tile, or in a frame that never got feedback, is
resent up to MAX_TRIES times; beyond that the loss is left to the transport (TCP). The queue is
bounded (tail drop) and identical packets still waiting are dropped (TCP's 1 s SYN retries vs a
~15 s round trip would otherwise pile up).
"""
import hashlib
import threading
import time
from collections import deque
from typing import Callable, List

import numpy as np

FRAG_HDR = 8
MAX_TRIES = 3
MAX_QUEUE_BYTES = 64 * 1024
REASSEMBLY_TIMEOUT_S = 180


class PacketQueue:
    """Outgoing side: fill(seq, caps) / ack(seq, ok) / requeue_before(seq), thread-safe push()."""

    def __init__(self, seed: int = 0):
        self.rng = np.random.default_rng(seed)
        self.lock = threading.Lock()
        self.packets = {}                 # pid -> bytes
        self.ranges = deque()             # (pid, offset, length, tries) waiting to be sent
        self.inflight = {}                # seq -> [(tile, pid, offset, length, tries)]
        self.waiting = {}                 # packet hash -> pid, for packets not yet fully sent
        self.next_pid = 1
        self.stats = {"queued": 0, "dropped_full": 0, "dropped_dup": 0, "given_up": 0}

    def push(self, pkt: bytes):
        h = hashlib.blake2b(pkt, digest_size=8).digest()
        with self.lock:
            if h in self.waiting:
                self.stats["dropped_dup"] += 1
                return
            if sum(r[2] for r in self.ranges) + len(pkt) > MAX_QUEUE_BYTES:
                self.stats["dropped_full"] += 1
                return
            pid = self.next_pid
            self.next_pid = self.next_pid % 0xFFFF + 1
            self.packets[pid] = pkt
            self.waiting[h] = pid
            self.ranges.append((pid, 0, len(pkt), 0))
            self.stats["queued"] += 1

    @property
    def pending_bytes(self) -> int:
        with self.lock:
            return sum(r[2] for r in self.ranges)

    def fill(self, seq: int, caps: List[int]) -> List[bytes]:
        out = [b""] * len(caps)
        sent = []
        order = sorted(range(len(caps)), key=lambda t: (-caps[t], self.rng.random()))
        with self.lock:
            for tile in order:
                room, block = caps[tile], bytearray()
                while self.ranges and room - len(block) > FRAG_HDR:
                    pid, off, n, tries = self.ranges.popleft()
                    take = min(n, room - len(block) - FRAG_HDR)
                    pkt = self.packets[pid]
                    block += (pid.to_bytes(2, "big") + off.to_bytes(2, "big") + take.to_bytes(2, "big")
                              + len(pkt).to_bytes(2, "big") + pkt[off:off + take])
                    sent.append((tile, pid, off, take, tries))
                    if take < n:                                   # rest of this range goes on
                        self.ranges.appendleft((pid, off + take, n - take, tries))
                if block:
                    out[tile] = bytes(block)
            self.inflight[seq] = sent
            self._forget_unreferenced()
        return out

    def ack(self, seq: int, ok: np.ndarray):
        with self.lock:
            retry = []
            for tile, pid, off, n, tries in self.inflight.pop(seq, []):
                if tile < len(ok) and ok[tile]:
                    continue
                if tries + 1 < MAX_TRIES:
                    retry.append((pid, off, n, tries + 1))
                else:
                    self.stats["given_up"] += 1
            self.ranges.extendleft(reversed(retry))                # retransmissions first
            self._forget_unreferenced()

    def requeue_before(self, seq: int):
        for s in [s for s in list(self.inflight) if s < seq]:
            self.ack(s, np.zeros(0, bool))

    def _forget_unreferenced(self):
        live = {r[0] for r in self.ranges} | {f[1] for fs in self.inflight.values() for f in fs}
        for pid in [p for p in self.packets if p not in live]:
            del self.packets[pid]
        queued = {r[0] for r in self.ranges if r[1] == 0}
        for h in [h for h, p in self.waiting.items() if p not in queued]:
            del self.waiting[h]


class PacketReassembly:
    """Incoming side: put(block) parses fragments; complete packets go to `deliver` once."""

    def __init__(self, deliver: Callable[[bytes], None]):
        self.deliver = deliver
        self.partial = {}                 # (pid, length) -> [buffer, have mask, first seen]
        self.recent = deque(maxlen=2048)  # recently delivered (pid, length): ignore retransmits
        self.stats = {"delivered": 0, "expired": 0}

    def put(self, block: bytes):
        i = 0
        while i + FRAG_HDR <= len(block):
            pid = int.from_bytes(block[i:i + 2], "big")
            if pid == 0:
                break
            off, n, total = (int.from_bytes(block[i + j:i + j + 2], "big") for j in (2, 4, 6))
            data = block[i + FRAG_HDR:i + FRAG_HDR + n]
            i += FRAG_HDR + n
            key = (pid, total)
            if key in self.recent or off + len(data) > total:
                continue
            buf, have, _ = self.partial.setdefault(key, [bytearray(total), np.zeros(total, bool), time.time()])
            buf[off:off + len(data)] = data
            have[off:off + len(data)] = True
            if have.all():
                del self.partial[key]
                self.recent.append(key)
                self.stats["delivered"] += 1
                self.deliver(bytes(buf))
        now = time.time()
        for key in [k for k, v in self.partial.items() if now - v[2] > REASSEMBLY_TIMEOUT_S]:
            del self.partial[key]
            self.stats["expired"] += 1
