"""The changing-noise scenario the link must adapt to (shared by simulation and over-the-air runs).

SIR is relative to the whole received signal; the signal spans ~9 kHz, so a 2-3 kHz jammer at
-3 dB is ~-9 dB inside its band (those tiles die) while the rest of the band is untouched.
Flat broadband noise hurts more than its SIR suggests (the signal is concentrated in the strong
parts of the band): at +8 dB most tiles fall below ~4 dB and the link drops; +14 dB is survivable.
"""
from .channel import Interferer

PHASES = [
    ("quiet", []),
    ("jam 2-4 kHz", [Interferer(2000, 4000, -3)]),
    ("jam 6-9 kHz", [Interferer(6000, 9000, -3)]),
    ("broadband +14dB", [Interferer(800, 10000, 14)]),
    ("bursts 1-5 kHz", [Interferer(1000, 5000, -3, duty=0.4)]),
    ("quiet again", []),
]


def schedule(frames_per_phase: int):
    """[(phase index, phase name, interferers)] per frame."""
    return [(i, name, its) for i, (name, its) in enumerate(PHASES) for _ in range(frames_per_phase)]
