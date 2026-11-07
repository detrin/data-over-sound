#!/usr/bin/env python3
"""
Sound Packet Bridge

Moves framed packets between Python queues and minimodem, half-duplex over the air:
each outgoing frame is played by a short-lived `minimodem -t` (its exit marks the end of
playback), and a `minimodem -r` listener runs whenever we are not transmitting.

On the wire every frame is base64 text between BEGIN_DATA / END_DATA markers, so the
receiver can find frame boundaries in minimodem's byte stream.
"""

import base64
import binascii
import math
import os
import queue
import select
import subprocess
import threading
import time
from typing import List, Optional, Sequence

BEGIN = b"BEGIN_DATA\n"
END = b"\nEND_DATA\n"
BITS_PER_CHAR = 10          # 8-N-1
LEAD_S = 1.0                # minimodem carrier lead-in + audio stack latency
MAX_RX_BUFFER = 16384


def wrap(frame: bytes) -> bytes:
    return BEGIN + base64.b64encode(frame) + END


def airtime(frame_len: int, baud: float) -> float:
    """Seconds needed to play one frame of `frame_len` bytes."""
    chars = len(BEGIN) + 4 * math.ceil(frame_len / 3) + len(END)
    return LEAD_S + chars * BITS_PER_CHAR / baud


def extract_frames(buffer: bytearray) -> List[bytes]:
    """Pop every complete BEGIN..END frame out of `buffer` (in place) and return the decoded frames."""
    frames = []
    while True:
        start = buffer.find(BEGIN)
        if start < 0:
            del buffer[:max(0, len(buffer) - len(BEGIN))]   # keep a possible partial marker
            return frames
        end = buffer.find(END, start + len(BEGIN))
        if end < 0:
            del buffer[:start]
            return frames
        body = bytes(buffer[start + len(BEGIN):end])
        del buffer[:end + len(END)]
        try:
            frames.append(base64.b64decode(body))
        except (binascii.Error, ValueError):
            print(f"Corrupted base64 frame ({len(body)} chars), dropping")


class MinimodemBridge:
    """Bridge between packet queues and minimodem (half-duplex)."""

    def __init__(self,
                 minimodem_path: str,
                 tx_queue: queue.Queue,
                 rx_queue: queue.Queue,
                 baud_rate: float = 100,
                 mark: Optional[float] = None,
                 space: Optional[float] = None,
                 volume: Optional[float] = None,
                 turnaround: float = 1.0):
        """
        Args:
            minimodem_path: minimodem executable
            tx_queue: frames to transmit
            rx_queue: received frames are put here
            baud_rate: minimodem baud rate
            mark, space: tone frequencies in Hz (minimodem defaults if None)
            volume: minimodem TX amplitude 0..1 (minimodem default if None)
            turnaround: pause before transmitting so the peer can switch back to RX
        """
        self.minimodem_path = minimodem_path
        self.tx_queue = tx_queue
        self.rx_queue = rx_queue
        self.baud_rate = baud_rate
        self.turnaround = turnaround
        self.modem_args = self._modem_args(baud_rate, mark, space)
        self.tx_args = ["--volume", str(volume)] if volume is not None else []
        self.rx_process: Optional[subprocess.Popen] = None
        self.rx_buffer = bytearray()
        self.running = False
        self.thread: Optional[threading.Thread] = None

    @staticmethod
    def _modem_args(baud: float, mark: Optional[float], space: Optional[float]) -> List[str]:
        args = ["--quiet"]
        if mark is not None:
            args += ["--mark", str(mark)]
        if space is not None:
            args += ["--space", str(space)]
        return args + [str(baud)]

    # --- lifecycle ------------------------------------------------------
    def start(self):
        """Start listening and processing in a background thread."""
        self.running = True
        self._start_rx()
        self.thread = threading.Thread(target=self._loop, daemon=True)
        self.thread.start()

    def stop(self):
        self.running = False
        if self.thread:
            self.thread.join(timeout=2)
        self._stop_rx()

    def is_alive(self) -> bool:
        return bool(self.thread and self.thread.is_alive())

    def _loop(self):
        while self.running:
            try:
                frame = self.tx_queue.get(timeout=0.05)
            except queue.Empty:
                self._read_rx()
                continue
            self._transmit(frame)

    # --- TX -------------------------------------------------------------
    def _transmit(self, frame: bytes):
        self._sleep_listening(self.turnaround)
        self._stop_rx()
        payload = wrap(frame)
        print(f"TX {len(frame)} bytes (~{airtime(len(frame), self.baud_rate):.1f}s on air)")
        result = self._run_minimodem(["--tx"] + self.tx_args, payload)
        if result.returncode != 0:
            print(f"minimodem TX failed ({result.returncode}): {result.stderr.decode(errors='ignore').strip()}")
        self._start_rx()

    def _sleep_listening(self, seconds: float):
        deadline = time.time() + seconds
        while time.time() < deadline:
            self._read_rx()

    def _run_minimodem(self, mode_args: Sequence[str], stdin: bytes) -> subprocess.CompletedProcess:
        return subprocess.run([self.minimodem_path, *mode_args, *self.modem_args],
                              input=stdin, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)

    # --- RX -------------------------------------------------------------
    def _start_rx(self):
        self.rx_buffer.clear()
        self.rx_process = subprocess.Popen([self.minimodem_path, "--rx", *self.modem_args],
                                           stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
        os.set_blocking(self.rx_process.stdout.fileno(), False)

    def _stop_rx(self):
        if self.rx_process:
            self.rx_process.terminate()
            try:
                self.rx_process.wait(timeout=2)
            except subprocess.TimeoutExpired:
                self.rx_process.kill()
            self.rx_process = None

    def _read_rx(self):
        if not self.rx_process:
            return
        if self.rx_process.poll() is not None:
            print("minimodem RX exited, restarting")
            self._start_rx()
            return
        out = self.rx_process.stdout
        if not select.select([out], [], [], 0.05)[0]:
            return
        chunk = out.read(4096)
        if not chunk:
            return
        self.rx_buffer.extend(chunk)
        for frame in extract_frames(self.rx_buffer):
            print(f"RX frame {len(frame)} bytes")
            self.rx_queue.put(frame)
        if len(self.rx_buffer) > MAX_RX_BUFFER:
            self.rx_buffer.clear()
