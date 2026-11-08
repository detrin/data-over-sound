//! The changing-noise scenario the link must adapt to (simulation and over-the-air runs).
//! SIR is relative to the whole received signal; a 2-3 kHz jammer at -3 dB is ~-9 dB inside its
//! band. Flat broadband noise at +8 dB breaks every strategy; +14 dB is survivable.

use crate::channel::Interferer;

pub fn phases() -> Vec<(&'static str, Vec<Interferer>)> {
    vec![
        ("quiet", vec![]),
        ("jam 2-4 kHz", vec![Interferer::new(2000.0, 4000.0, -3.0)]),
        ("jam 6-9 kHz", vec![Interferer::new(6000.0, 9000.0, -3.0)]),
        ("broadband +14dB", vec![Interferer::new(800.0, 10000.0, 14.0)]),
        ("bursts 1-5 kHz", vec![Interferer { f_lo: 1000.0, f_hi: 5000.0, sir_db: -3.0, duty: 0.4 }]),
        ("quiet again", vec![]),
    ]
}
