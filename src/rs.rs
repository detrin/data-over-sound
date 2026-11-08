//! Reed-Solomon over GF(2^8), compatible with the Python `reedsolo` defaults
//! (primitive polynomial 0x11d, generator 2, first consecutive root 0, systematic: message || parity).

use std::sync::OnceLock;

struct Gf {
    exp: [u8; 512],
    log: [u8; 256],
}

fn gf() -> &'static Gf {
    static GF: OnceLock<Gf> = OnceLock::new();
    GF.get_or_init(|| {
        let mut exp = [0u8; 512];
        let mut log = [0u8; 256];
        let mut x: u16 = 1;
        for i in 0..255 {
            exp[i] = x as u8;
            log[x as usize] = i as u8;
            x <<= 1;
            if x & 0x100 != 0 {
                x ^= 0x11d;
            }
        }
        for i in 255..512 {
            exp[i] = exp[i - 255];
        }
        Gf { exp, log }
    })
}

#[inline]
fn mul(a: u8, b: u8) -> u8 {
    if a == 0 || b == 0 {
        return 0;
    }
    let g = gf();
    g.exp[g.log[a as usize] as usize + g.log[b as usize] as usize]
}

#[inline]
fn div(a: u8, b: u8) -> u8 {
    assert!(b != 0, "GF division by zero");
    if a == 0 {
        return 0;
    }
    let g = gf();
    g.exp[(g.log[a as usize] as usize + 255 - g.log[b as usize] as usize) % 255]
}

#[inline]
fn pow_alpha(e: i64) -> u8 {
    gf().exp[e.rem_euclid(255) as usize]
}

/// Evaluate a big-endian polynomial (highest degree first) at x.
fn eval_be(p: &[u8], x: u8) -> u8 {
    p.iter().fold(0u8, |acc, &c| mul(acc, x) ^ c)
}

/// Evaluate a little-endian polynomial (constant term first) at x.
fn eval_le(p: &[u8], x: u8) -> u8 {
    p.iter().rev().fold(0u8, |acc, &c| mul(acc, x) ^ c)
}

fn generator_poly(nsym: usize) -> Vec<u8> {
    // big endian: prod_{i<nsym} (x - alpha^i)
    let mut g = vec![1u8];
    for i in 0..nsym {
        let r = pow_alpha(i as i64);
        let mut next = vec![0u8; g.len() + 1];
        for (j, &c) in g.iter().enumerate() {
            next[j] ^= c;
            next[j + 1] ^= mul(c, r);
        }
        g = next;
    }
    g
}

/// Systematic encoding: returns message followed by `nsym` parity bytes.
pub fn encode(msg: &[u8], nsym: usize) -> Vec<u8> {
    assert!(msg.len() + nsym <= 255, "RS codeword longer than 255");
    let g = generator_poly(nsym);
    let mut buf = msg.to_vec();
    buf.extend(std::iter::repeat_n(0, nsym));
    for i in 0..msg.len() {
        let coef = buf[i];
        if coef != 0 {
            for j in 1..g.len() {
                buf[i + j] ^= mul(g[j], coef);
            }
        }
    }
    let mut out = msg.to_vec();
    out.extend_from_slice(&buf[msg.len()..]);
    out
}

fn syndromes(cw: &[u8], nsym: usize) -> Vec<u8> {
    (0..nsym).map(|j| eval_be(cw, pow_alpha(j as i64))).collect()
}

/// Decode a codeword (message || parity). Returns the corrected message, or None when the errors
/// cannot be corrected (more than nsym/2 symbol errors, detected).
pub fn decode(cw: &[u8], nsym: usize) -> Option<Vec<u8>> {
    let n = cw.len();
    if n <= nsym {
        return None;
    }
    let s = syndromes(cw, nsym);
    if s.iter().all(|&x| x == 0) {
        return Some(cw[..n - nsym].to_vec());
    }
    // Berlekamp-Massey (little-endian locator, lambda[0] = 1)
    let mut lambda = vec![1u8];
    let mut prev = vec![1u8];
    let (mut l, mut m, mut b) = (0usize, 1usize, 1u8);
    for k in 0..nsym {
        let mut d = s[k];
        for i in 1..=l.min(lambda.len() - 1) {
            d ^= mul(lambda[i], s[k - i]);
        }
        if d == 0 {
            m += 1;
            continue;
        }
        let coef = div(d, b);
        let mut next = lambda.clone();
        if next.len() < prev.len() + m {
            next.resize(prev.len() + m, 0);
        }
        for (i, &p) in prev.iter().enumerate() {
            next[i + m] ^= mul(coef, p);
        }
        if 2 * l <= k {
            prev = lambda;
            l = k + 1 - l;
            b = d;
            m = 1;
        } else {
            m += 1;
        }
        lambda = next;
    }
    while lambda.len() > 1 && *lambda.last().unwrap() == 0 {
        lambda.pop();
    }
    let n_err = lambda.len() - 1;
    if n_err == 0 || 2 * n_err > nsym || n_err != l {
        return None;
    }
    // Chien search: error at byte k has locator X = alpha^(n-1-k); root at X^-1
    let mut pos = Vec::with_capacity(n_err);
    for k in 0..n {
        let xinv = pow_alpha(-((n - 1 - k) as i64));
        if eval_le(&lambda, xinv) == 0 {
            pos.push(k);
        }
    }
    if pos.len() != n_err {
        return None;
    }
    // Forney (first root 0): e = X * Omega(X^-1) / Lambda'(X^-1), Omega = S * Lambda mod x^nsym
    let mut omega = vec![0u8; nsym];
    for (i, &si) in s.iter().enumerate() {
        for (j, &lj) in lambda.iter().enumerate() {
            if i + j < nsym {
                omega[i + j] ^= mul(si, lj);
            }
        }
    }
    let deriv: Vec<u8> = lambda.iter().enumerate().skip(1).map(|(i, &c)| if i % 2 == 1 { c } else { 0 }).collect();
    let mut out = cw.to_vec();
    for &k in &pos {
        let x = pow_alpha((n - 1 - k) as i64);
        let xinv = pow_alpha(-((n - 1 - k) as i64));
        let den = eval_le(&deriv, xinv);
        if den == 0 {
            return None;
        }
        out[k] ^= mul(x, div(eval_le(&omega, xinv), den));
    }
    if syndromes(&out, nsym).iter().any(|&x| x != 0) {
        return None;
    }
    out.truncate(n - nsym);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_correction() {
        let msg: Vec<u8> = (0..100u8).map(|i| i.wrapping_mul(37).wrapping_add(11)).collect();
        let cw = encode(&msg, 24);
        assert_eq!(decode(&cw, 24).unwrap(), msg);
        let mut bad = cw.clone();
        for i in 0..12 {
            bad[i * 9] ^= 0x5a ^ i as u8;
        }
        assert_eq!(decode(&bad, 24).unwrap(), msg);
        let mut worse = cw;
        for i in 0..20 {
            worse[i * 5] ^= 0xa5;
        }
        assert!(decode(&worse, 24).map_or(true, |m| m != msg));
    }
}
