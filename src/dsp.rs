//! Signal-processing helpers: cached real FFTs, cross-correlation, chirp, windows, PSD, filters.

use num_complex::Complex64;
use realfft::{RealFftPlanner, RealToComplex, ComplexToReal};
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

thread_local! {
    static PLANNER: RefCell<RealFftPlanner<f64>> = RefCell::new(RealFftPlanner::new());
    static FWD: RefCell<HashMap<usize, Arc<dyn RealToComplex<f64>>>> = RefCell::new(HashMap::new());
    static INV: RefCell<HashMap<usize, Arc<dyn ComplexToReal<f64>>>> = RefCell::new(HashMap::new());
}

fn fwd_plan(n: usize) -> Arc<dyn RealToComplex<f64>> {
    FWD.with(|m| m.borrow_mut().entry(n).or_insert_with(|| PLANNER.with(|p| p.borrow_mut().plan_fft_forward(n))).clone())
}

fn inv_plan(n: usize) -> Arc<dyn ComplexToReal<f64>> {
    INV.with(|m| m.borrow_mut().entry(n).or_insert_with(|| PLANNER.with(|p| p.borrow_mut().plan_fft_inverse(n))).clone())
}

/// numpy.fft.rfft (unnormalised).
pub fn rfft(x: &[f64]) -> Vec<Complex64> {
    let plan = fwd_plan(x.len());
    let mut input = x.to_vec();
    let mut out = plan.make_output_vec();
    plan.process(&mut input, &mut out).expect("rfft");
    out
}

/// numpy.fft.irfft(spec, n) (normalised by 1/n). DC and Nyquist imaginary parts are ignored.
pub fn irfft(spec: &[Complex64], n: usize) -> Vec<f64> {
    let plan = inv_plan(n);
    let mut s = spec.to_vec();
    s[0].im = 0.0;
    if n.is_multiple_of(2) {
        let last = s.len() - 1;
        s[last].im = 0.0;
    }
    let mut out = plan.make_output_vec();
    plan.process(&mut s, &mut out).expect("irfft");
    let k = 1.0 / n as f64;
    out.iter_mut().for_each(|v| *v *= k);
    out
}

/// Smallest 2^a 3^b 5^c >= n (fast FFT length).
pub fn next_fast_len(n: usize) -> usize {
    let mut best = usize::MAX;
    let mut p2 = 1usize;
    while p2 < 2 * n.max(1) {
        let mut p3 = p2;
        while p3 < 2 * n.max(1) {
            let mut p5 = p3;
            while p5 < n {
                p5 *= 5;
            }
            best = best.min(p5);
            p3 *= 3;
        }
        p2 *= 2;
    }
    best
}

/// |scipy.signal.fftconvolve(x, h[::-1], mode="valid")|: sliding correlation of x with h.
pub fn xcorr_valid_abs(x: &[f64], h: &[f64]) -> Vec<f64> {
    if x.len() < h.len() {
        return vec![];
    }
    let n = next_fast_len(x.len() + h.len());
    let mut xp = x.to_vec();
    xp.resize(n, 0.0);
    let mut hp = h.to_vec();
    hp.resize(n, 0.0);
    let xs = rfft(&xp);
    let hs = rfft(&hp);
    let prod: Vec<Complex64> = xs.iter().zip(&hs).map(|(a, b)| a * b.conj()).collect();
    let c = irfft(&prod, n);
    c[..x.len() - h.len() + 1].iter().map(|v| v.abs()).collect()
}

/// scipy.signal.fftconvolve(x, h) (full linear convolution).
pub fn convolve(x: &[f64], h: &[f64]) -> Vec<f64> {
    let len = x.len() + h.len() - 1;
    let n = next_fast_len(len);
    let mut xp = x.to_vec();
    xp.resize(n, 0.0);
    let mut hp = h.to_vec();
    hp.resize(n, 0.0);
    let xs = rfft(&xp);
    let hs = rfft(&hp);
    let prod: Vec<Complex64> = xs.iter().zip(&hs).map(|(a, b)| a * b).collect();
    let mut c = irfft(&prod, n);
    c.truncate(len);
    c
}

/// scipy.signal.chirp(t, f0, t1, f1) (linear) sampled at fs for n samples, t1 = (n-1)/fs.
pub fn chirp(n: usize, fs: f64, f0: f64, f1: f64) -> Vec<f64> {
    let t1 = (n - 1) as f64 / fs;
    let beta = (f1 - f0) / t1;
    (0..n)
        .map(|i| {
            let t = i as f64 / fs;
            (2.0 * std::f64::consts::PI * (f0 * t + 0.5 * beta * t * t)).cos()
        })
        .collect()
}

/// numpy.hanning(n).
pub fn hanning(n: usize) -> Vec<f64> {
    if n == 1 {
        return vec![1.0];
    }
    (0..n).map(|i| 0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / (n - 1) as f64).cos()).collect()
}

/// numpy.median.
pub fn median(x: &[f64]) -> f64 {
    let mut v: Vec<f64> = x.iter().copied().filter(|v| !v.is_nan()).collect();
    if v.is_empty() {
        return f64::NAN;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let m = v.len() / 2;
    if v.len() % 2 == 1 { v[m] } else { 0.5 * (v[m - 1] + v[m]) }
}

pub fn rms(x: &[f64]) -> f64 {
    if x.is_empty() {
        return 0.0;
    }
    (x.iter().map(|v| v * v).sum::<f64>() / x.len() as f64).sqrt()
}

/// RMS of x restricted to [f_lo, f_hi] (brick-wall FFT filter).
pub fn band_rms(x: &[f64], fs: f64, f_lo: f64, f_hi: f64) -> f64 {
    let n = x.len();
    if n == 0 {
        return 0.0;
    }
    let s = rfft(x);
    let df = fs / n as f64;
    let mut e = 0.0;
    for (k, c) in s.iter().enumerate() {
        let f = k as f64 * df;
        if f >= f_lo && f <= f_hi {
            let w = if k == 0 || (n.is_multiple_of(2) && k == s.len() - 1) { 1.0 } else { 2.0 };
            e += w * c.norm_sqr();
        }
    }
    (e / (n as f64 * n as f64)).sqrt()
}

/// Welch power spectral density (Hann window, 50% overlap), one-sided, per Hz: (freqs, psd).
pub fn welch(x: &[f64], fs: f64, nperseg: usize) -> (Vec<f64>, Vec<f64>) {
    let w = hanning_periodic(nperseg);
    let wss: f64 = w.iter().map(|v| v * v).sum();
    let step = nperseg / 2;
    let mut acc = vec![0.0; nperseg / 2 + 1];
    let mut count = 0usize;
    let mut start = 0;
    while start + nperseg <= x.len() {
        let seg = &x[start..start + nperseg];
        let mean = seg.iter().sum::<f64>() / nperseg as f64;
        let win: Vec<f64> = seg.iter().zip(&w).map(|(v, w)| (v - mean) * w).collect();
        for (a, c) in acc.iter_mut().zip(rfft(&win)) {
            *a += c.norm_sqr();
        }
        count += 1;
        start += step;
    }
    let scale = 1.0 / (fs * wss * count.max(1) as f64);
    let last = acc.len() - 1;
    let psd: Vec<f64> = acc
        .iter()
        .enumerate()
        .map(|(k, v)| v * scale * if k == 0 || (nperseg.is_multiple_of(2) && k == last) { 1.0 } else { 2.0 })
        .collect();
    let f = (0..acc.len()).map(|k| k as f64 * fs / nperseg as f64).collect();
    (f, psd)
}

fn hanning_periodic(n: usize) -> Vec<f64> {
    (0..n).map(|i| 0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / n as f64).cos()).collect()
}

/// Linear interpolation like numpy.interp (xp ascending).
pub fn interp(x: f64, xp: &[f64], fp: &[f64]) -> f64 {
    if x <= xp[0] {
        return fp[0];
    }
    if x >= xp[xp.len() - 1] {
        return fp[fp.len() - 1];
    }
    let i = xp.partition_point(|&v| v <= x);
    let (x0, x1, y0, y1) = (xp[i - 1], xp[i], fp[i - 1], fp[i]);
    y0 + (y1 - y0) * (x - x0) / (x1 - x0)
}
