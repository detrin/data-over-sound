"""Real-time audio through PipeWire's command-line tools (no extra Python audio library).

Recorder: one long-running `pw-record` writing raw s16 mono to a pipe; a thread appends samples to
a buffer indexed by absolute sample number. play(): one `pw-play` per frame (it exits when drained).
"""
import os
import subprocess
import tempfile
import threading

import numpy as np
from scipy.io import wavfile

FS = 48000
RAW = ["--rate", str(FS), "--channels", "1", "--format", "s16"]


class Recorder:
    def __init__(self, keep_s: float = 60.0):
        self.proc = subprocess.Popen(["pw-record", *RAW, "-"], stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
        self.keep = int(keep_s * FS)
        self.lock = threading.Lock()
        self.buf = np.zeros(0, np.float32)
        self.start = 0                          # absolute index of buf[0]
        threading.Thread(target=self._pump, daemon=True).start()

    def _pump(self):
        while True:
            raw = self.proc.stdout.read(FS // 10 * 2)
            if not raw:
                return
            x = np.frombuffer(raw[: len(raw) // 2 * 2], np.int16).astype(np.float32) / 32768
            with self.lock:
                self.buf = np.concatenate([self.buf, x])
                if len(self.buf) > self.keep:
                    drop = len(self.buf) - self.keep
                    self.buf, self.start = self.buf[drop:], self.start + drop

    @property
    def now(self) -> int:
        """Absolute index one past the newest sample."""
        with self.lock:
            return self.start + len(self.buf)

    def since(self, idx: int) -> np.ndarray:
        with self.lock:
            return self.buf[max(0, idx - self.start):].astype(np.float64)

    def close(self):
        self.proc.terminate()


def play(x: np.ndarray):
    """Blocking: returns when PipeWire has played the whole buffer. Goes through a temporary WAV
    file: newer pw-play versions refuse raw audio on stdin."""
    with tempfile.NamedTemporaryFile(suffix=".wav", dir="/dev/shm" if os.path.isdir("/dev/shm") else None) as f:
        wavfile.write(f.name, FS, (np.clip(x, -1, 1) * 32767).astype(np.int16))
        subprocess.run(["pw-play", f.name], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, check=True)
