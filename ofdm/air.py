"""Over-the-air I/O: play a WAV on one laptop while recording on the other (optionally with an interferer).

The laptops only play/record; all DSP runs locally. Hosts are ssh aliases with a PipeWire desktop session.
"""
import os
import subprocess
import tempfile
import time
from typing import Optional

import numpy as np
from scipy.io import wavfile

FS = 48000
ENV = "export XDG_RUNTIME_DIR=/run/user/1000;"
HOSTS = ("dell-g5", "e540")


def peer(host: str) -> str:
    return HOSTS[1] if host == HOSTS[0] else HOSTS[0]


def write_wav(path: str, x: np.ndarray):
    wavfile.write(path, FS, (np.clip(x, -1, 1) * 32767).astype(np.int16))


def read_wav(path: str) -> np.ndarray:
    rate, x = wavfile.read(path)
    assert rate == FS, rate
    x = x.astype(np.float64) / 32768
    return x if x.ndim == 1 else x.mean(axis=1)


def _ssh(host: str, cmd: str, **kw) -> subprocess.Popen:
    return subprocess.Popen(["ssh", host, ENV + cmd], stdout=subprocess.PIPE, stderr=subprocess.PIPE, **kw)


def _push(host: str, local: str, remote: str):
    subprocess.run(["scp", "-q", local, f"{host}:{remote}"], check=True)


def transmit(tx: str, signal: np.ndarray, interferer: Optional[np.ndarray] = None,
             lead_s: float = 0.8, tail_s: float = 0.8) -> np.ndarray:
    """Play `signal` on host `tx`, record on its peer; optionally the receiver plays `interferer`
    through its own speaker while recording (real acoustic noise). Returns the recording."""
    rx = peer(tx)
    dur = len(signal) / FS + lead_s + tail_s + 0.6
    with tempfile.TemporaryDirectory() as d:
        write_wav(f"{d}/tx.wav", signal)
        _push(tx, f"{d}/tx.wav", "/tmp/ofdm_tx.wav")
        play_noise = ""
        if interferer is not None:
            write_wav(f"{d}/noise.wav", interferer)
            _push(rx, f"{d}/noise.wav", "/tmp/ofdm_noise.wav")
            play_noise = "pw-play /tmp/ofdm_noise.wav & "
        rec = _ssh(rx, f"{play_noise}timeout {dur:.1f} pw-record --rate {FS} --channels 1 --format s16 /tmp/ofdm_rx.wav; wait")
        time.sleep(lead_s + 0.6)   # recorder warm-up (ssh + PipeWire stream start)
        play = _ssh(tx, "pw-play /tmp/ofdm_tx.wav")
        play.wait(); rec.wait()
        subprocess.run(["scp", "-q", f"{rx}:/tmp/ofdm_rx.wav", f"{d}/rx.wav"], check=True)
        return read_wav(f"{d}/rx.wav")


def record_silence(host: str, seconds: float) -> np.ndarray:
    with tempfile.TemporaryDirectory() as d:
        _ssh(host, f"timeout {seconds:.1f} pw-record --rate {FS} --channels 1 --format s16 /tmp/ofdm_sil.wav").wait()
        subprocess.run(["scp", "-q", f"{host}:/tmp/ofdm_sil.wav", f"{d}/s.wav"], check=True)
        return read_wav(f"{d}/s.wav")
