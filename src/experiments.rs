//! Research and verification tools: PHY check, room measurement, SNR-model calibration,
//! strategy comparison (simulation and over the air with broadcast noise), two-way file transfer.

use crate::air::{self, Hosts};
use crate::audio::FS;
use crate::channel::{AcousticChannel, ChannelData, Interferer};
use crate::controllers::{all_strategies, finalists, Controller};
use crate::dsp;
use crate::frame::Header;
use crate::link::{build_frame, receive_frame};
use crate::model;
use crate::node::Node;
use crate::phy::{Phy, PhyConfig};
use crate::scenario;
use crate::traffic::{FileReceiver, FileSender};
use rand::rngs::StdRng;
use rand::{RngCore, SeedableRng};
use std::collections::BTreeMap;

pub const CHANNELS: [&str; 2] = ["dell-g5_to_e540", "e540_to_dell-g5"];

fn random_bytes(rng: &mut StdRng) -> impl FnMut(usize) -> Vec<u8> + '_ {
    move |n| {
        let mut b = vec![0u8; n];
        rng.fill_bytes(&mut b);
        b
    }
}

fn fmt_snr(s: &[f64]) -> String {
    s.iter().map(|v| format!("{v:.0}")).collect::<Vec<_>>().join(" ")
}

/// Loopback and simulated-channel decoding at fixed levels.
pub fn check() -> anyhow::Result<()> {
    let phy = Phy::new(PhyConfig::default());
    let c = &phy.cfg;
    let mut rng = StdRng::seed_from_u64(0);
    println!("tiles={} bins={} frame={:.2}s header_bits={}", c.n_tiles(), c.n_bins(), c.frame_s(), c.header_bits());
    let fs = c.fs as f64;
    for name in ["loopback", CHANNELS[0], CHANNELS[1]] {
        let mut ch = if name == "loopback" { None } else { Some(AcousticChannel::new(name, 1)?) };
        for lev in 1..=3u8 {
            let levels = vec![lev; c.n_tiles()];
            let (wave, pl) = build_frame(&phy, &Header::new(lev as u16, levels.clone()), &levels, random_bytes(&mut rng));
            let rx = match ch.as_mut() {
                None => [vec![0.0; 5000], wave, vec![0.0; 5000]].concat(),
                Some(ch) => ch.apply(&wave, fs, &[]),
            };
            let r = receive_frame(&phy, &rx, None, Some(&pl));
            println!(
                "{name:16} L{lev}: hdr {} ok {:2}/{} {:5.0} bit/s drift {:5.1}ppm snr: {}",
                if r.header.is_some() { "ok " } else { "BAD" },
                r.tile_ok.iter().filter(|&&o| o).count(),
                c.n_tiles(),
                8.0 * r.payload_bytes as f64 / c.frame_s(),
                r.drift_ppm,
                fmt_snr(&r.tile_snr_db)
            );
        }
    }
    Ok(())
}

/// Exponential sine sweep (Farina) impulse response + noise PSD per direction -> channel JSON.
pub fn measure(hosts: &Hosts) -> anyhow::Result<()> {
    let (f1, f2, t, amp) = (200.0f64, 12000.0f64, 3.0f64, 0.2);
    let n = (t * FS as f64) as usize;
    let k = t / (f2 / f1).ln();
    let sweep: Vec<f64> = (0..n)
        .map(|i| {
            let ti = i as f64 / FS as f64;
            let fade = 1f64.min((ti / 0.05).min((t - ti) / 0.05));
            amp * fade * (2.0 * std::f64::consts::PI * f1 * k * ((ti / k).exp() - 1.0)).sin()
        })
        .collect();
    let inverse: Vec<f64> = (0..n)
        .map(|i| {
            let ti = i as f64 / FS as f64;
            let raw = (2.0 * std::f64::consts::PI * f1 * k * (((t - ti) / k).exp() - 1.0)).sin();
            raw * (-ti / k).exp()
        })
        .collect();
    for tx in hosts.both() {
        let rx = hosts.peer(&tx);
        let rec = air::transmit(hosts, &tx, &sweep, None)?;
        let h = dsp::convolve(&rec, &inverse);
        let peak = h.iter().enumerate().fold(0, |b, (i, v)| if v.abs() > h[b].abs() { i } else { b });
        let ir_raw = &h[peak.saturating_sub(200)..(peak + (0.8 * FS as f64) as usize).min(h.len())];
        let pk = ir_raw.iter().fold(0.0f64, |m, v| m.max(v.abs()));
        let ir: Vec<f64> = ir_raw.iter().map(|v| v / pk).collect();
        let sim = dsp::convolve(&sweep, &ir);
        let seg = &rec[peak.saturating_sub(n)..(peak + (0.3 * FS as f64) as usize).min(rec.len())];
        let gain = dsp::rms(seg) / dsp::rms(&sim[..seg.len().min(sim.len())]);
        let noise = air::record_silence(&rx, 4.0)?;
        let (f, psd) = dsp::welch(&noise, FS as f64, 4096);
        let data = ChannelData { ir: ir.clone(), gain, noise_f: f, noise_psd: psd, noise_rms: dsp::rms(&noise) };
        let name = format!("{tx}_to_{rx}");
        std::fs::write(ChannelData::path(&name), serde_json::to_string(&data)?)?;
        // energy arriving after each guard length relative to before it (reverb noise floor removed)
        let e: Vec<f64> = ir[200..].iter().map(|v| v * v).collect();
        let half = (0.5 * FS as f64) as usize;
        let floor = e[half.min(e.len() - 1)..].iter().sum::<f64>() / (e.len() - half.min(e.len() - 1)).max(1) as f64;
        let e: Vec<f64> = e[..half.min(e.len())].iter().map(|v| (v - floor).max(0.0)).collect();
        let isi: Vec<String> = [5, 10, 20, 40, 60, 160]
            .iter()
            .map(|&g| {
                let n = g * FS / 1000;
                format!("{g}:{:.0}", 10.0 * (e[n..].iter().sum::<f64>() / e[..n].iter().sum::<f64>() + 1e-12).log10())
            })
            .collect();
        let rec_peak = rec.iter().fold(0.0f64, |m, v| m.max(v.abs()));
        println!("{name}: rec peak {rec_peak:.2}, noise rms {:.4}, gain {:.4}; ISI floor per guard ms: {}", data.noise_rms, gain, isi.join(" "));
    }
    Ok(())
}

pub fn calibrate(frames_per_level: usize, out: &str) -> anyhow::Result<()> {
    let phy = Phy::new(PhyConfig::default());
    let rows = model::collect(&phy, frames_per_level, 0)?;
    let m = model::fit(&rows);
    std::fs::write(out, m.to_json())?;
    for l in 1..=3 {
        let lm = m.0[l];
        println!("L{l}: 50% at {:.1} dB, 90% at {:.1} dB (scale {:.2}, lapse {:.2}, n={})", lm.mid, lm.thr90, lm.scale, lm.lapse, lm.n);
    }
    println!("{} tile samples -> {out} (rebuild to embed it as the default)", rows.len());
    Ok(())
}

fn print_table(title: &str, rows: &BTreeMap<String, (Vec<f64>, f64)>) {
    let phases = scenario::phases();
    println!("{title}");
    print!("{:22}", "strategy");
    for (n, _) in &phases {
        print!("{:>16}", &n[..n.len().min(15)]);
    }
    println!("{:>9}{:>6}", "MEAN", "lost");
    let mut sorted: Vec<_> = rows.iter().collect();
    sorted.sort_by(|a, b| {
        let ma: f64 = a.1 .0.iter().sum::<f64>();
        let mb: f64 = b.1 .0.iter().sum::<f64>();
        mb.partial_cmp(&ma).unwrap()
    });
    for (name, (rates, lost)) in sorted {
        print!("{name:22}");
        for r in rates {
            print!("{r:16.0}");
        }
        println!("{:9.0}{lost:6.1}", rates.iter().sum::<f64>() / rates.len() as f64);
    }
}

/// One strategy through the whole scenario on one simulated direction: (bits per phase, lost frames).
fn run_strategy(phy: &Phy, ctrl: &mut dyn Controller, name: &str, frames_per_phase: usize, seed: u64) -> anyhow::Result<(Vec<f64>, f64, f64)> {
    let fs = phy.cfg.fs as f64;
    let phases = scenario::phases();
    let mut ch = AcousticChannel::new(name, seed)?;
    let mut rng = StdRng::seed_from_u64(seed);
    let (mut bits, mut lost, mut seq) = (vec![0.0; phases.len()], 0.0, 0u16);
    let (mut used, mut good) = (0usize, 0usize);
    for (pi, (_, its)) in phases.iter().enumerate() {
        for _ in 0..frames_per_phase {
            let levels = ctrl.choose();
            let (wave, pl) = build_frame(phy, &Header::new(seq, levels.clone()), &levels, random_bytes(&mut rng));
            let r = receive_frame(phy, &ch.apply(&wave, fs, its), None, Some(&pl));
            lost += r.header.is_none() as u8 as f64;
            ctrl.observe(&levels, &r.tile_ok, &r.tile_snr_db);
            bits[pi] += 8.0 * r.payload_bytes as f64;
            used += levels.iter().filter(|&&l| l > 0).count();
            good += r.tile_ok.iter().filter(|&&o| o).count();
            seq = seq.wrapping_add(1);
        }
    }
    Ok((bits, lost, good as f64 / used.max(1) as f64))
}

/// Strategy comparison in the calibrated simulator under the changing-noise scenario
/// (every strategy x direction x seed runs on its own thread).
pub fn simulate(frames_per_phase: usize, seeds: u64, only_finalists: bool) -> anyhow::Result<()> {
    let phy = Phy::new(PhyConfig::default());
    let n_phases = scenario::phases().len();
    let mut jobs = vec![];
    for name in CHANNELS {
        for seed in 0..seeds {
            let strategies: Vec<Box<dyn Controller>> = if only_finalists { finalists(&phy.cfg, seed) } else { all_strategies(&phy.cfg, seed) };
            jobs.extend(strategies.into_iter().map(|c| (name, seed, c)));
        }
    }
    let results: Vec<anyhow::Result<(String, Vec<f64>, f64, f64)>> = std::thread::scope(|sc| {
        let handles: Vec<_> = jobs
            .into_iter()
            .map(|(name, seed, mut ctrl)| {
                let phy = &phy;
                sc.spawn(move || {
                    let (bits, lost, tile_ok) = run_strategy(phy, ctrl.as_mut(), name, frames_per_phase, seed)?;
                    Ok((ctrl.name(), bits, lost, tile_ok))
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().expect("simulation thread")).collect()
    });
    let runs = (CHANNELS.len() as u64 * seeds) as f64;
    let air = frames_per_phase as f64 * phy.cfg.frame_s();
    let mut acc: BTreeMap<String, (Vec<f64>, f64)> = BTreeMap::new();
    let mut tile_ok: BTreeMap<String, f64> = BTreeMap::new();
    for r in results {
        let (name, bits, lost, ok) = r?;
        *tile_ok.entry(name.clone()).or_default() += ok / runs;
        let e = acc.entry(name).or_insert((vec![0.0; n_phases], 0.0));
        for (a, b) in e.0.iter_mut().zip(&bits) {
            *a += b / air / runs;
        }
        e.1 += lost / runs;
    }
    print_table(&format!("goodput bit/s per phase (simulation, mean over 2 directions x {seeds} seeds)"), &acc);
    println!("tile success rate: {}", tile_ok.iter().map(|(n, v)| format!("{n} {:.0}%", 100.0 * v)).collect::<Vec<_>>().join(", "));
    Ok(())
}

/// (received signal rms, receiver self-noise gain) for tx -> peer, used to set broadcast noise.
fn calibrate_noise(hosts: &Hosts, tx: &str, phy: &Phy, rng: &mut StdRng) -> anyhow::Result<(f64, f64)> {
    let c = &phy.cfg;
    let levels = vec![2u8; c.n_tiles()];
    let (wave, _) = build_frame(phy, &Header::new(0, levels.clone()), &levels, random_bytes(rng));
    let rec = air::transmit(hosts, tx, &wave, None)?;
    let a = (1.4 * FS as f64) as usize;
    let b = ((1.4 + c.frame_s() - 0.5) * FS as f64) as usize;
    let sig = dsp::band_rms(&rec[a.min(rec.len())..b.min(rec.len())], FS as f64, c.f_lo, c.f_hi);
    let probe = 0.01;
    let noise = Interferer::new(c.f_lo, c.f_hi, 0.0).render(4 * FS, FS as f64, probe, rng);
    let rec = air::transmit(hosts, tx, &vec![0.0; 2 * FS], Some(&noise))?;
    let gain = dsp::band_rms(&rec[FS.min(rec.len())..(3 * FS).min(rec.len())], FS as f64, c.f_lo, c.f_hi) / probe;
    Ok((sig, gain))
}

fn broadcast_noise(its: &[Interferer], phy: &Phy, cal: (f64, f64), rng: &mut StdRng) -> Option<Vec<f64>> {
    if its.is_empty() {
        return None;
    }
    let n = ((phy.cfg.frame_s() + 3.0) * FS as f64) as usize;
    let mut x = vec![0.0; n];
    for it in its {
        for (a, b) in x.iter_mut().zip(it.render(n, FS as f64, cal.0 / cal.1, rng)) {
            *a += b;
        }
    }
    Some(x)
}

/// Over-the-air strategy comparison with real broadcast noise; strategies round-robin frame by
/// frame inside each phase; paired difference to fixed-L3 with standard error.
pub fn compare_air(hosts: &Hosts, frames_per_phase: usize, only_finalists: bool) -> anyhow::Result<()> {
    let phy = Phy::new(PhyConfig::default());
    let phases = scenario::phases();
    let air_time = frames_per_phase as f64 * phy.cfg.frame_s();
    for tx in hosts.both() {
        let mut rng = StdRng::seed_from_u64(0);
        let cal = calibrate_noise(hosts, &tx, &phy, &mut rng)?;
        println!("[{tx}] received signal rms {:.4}, self-noise gain {:.2}", cal.0, cal.1);
        let mut strategies = if only_finalists { finalists(&phy.cfg, 0) } else { all_strategies(&phy.cfg, 0) };
        let mut table: BTreeMap<String, (Vec<f64>, f64)> = BTreeMap::new();
        let mut frames: BTreeMap<String, Vec<f64>> = BTreeMap::new();
        let mut seq = 0u16;
        for (pi, (pname, its)) in phases.iter().enumerate() {
            for rep in 0..frames_per_phase {
                for ctrl in strategies.iter_mut() {
                    let levels = ctrl.choose();
                    let (wave, pl) = build_frame(&phy, &Header::new(seq, levels.clone()), &levels, random_bytes(&mut rng));
                    let noise = broadcast_noise(its, &phy, cal, &mut rng);
                    let rec = air::transmit(hosts, &tx, &wave, noise.as_deref())?;
                    let r = receive_frame(&phy, &rec, None, Some(&pl));
                    ctrl.observe(&levels, &r.tile_ok, &r.tile_snr_db);
                    let rate = 8.0 * r.payload_bytes as f64 / phy.cfg.frame_s();
                    let e = table.entry(ctrl.name()).or_insert((vec![0.0; phases.len()], 0.0));
                    e.0[pi] += 8.0 * r.payload_bytes as f64 / air_time;
                    e.1 += r.header.is_none() as u8 as f64;
                    frames.entry(ctrl.name()).or_default().push(rate);
                    println!(
                        "[{tx}] {pname:16} #{rep} {:20} ok {:2}/{} {rate:6.0} bit/s hdr {}",
                        ctrl.name(),
                        r.tile_ok.iter().filter(|&&o| o).count(),
                        phy.cfg.n_tiles(),
                        if r.header.is_some() { "ok" } else { "LOST" }
                    );
                    seq += 1;
                }
            }
        }
        print_table(&format!("== {tx} -> {}", hosts.peer(&tx)), &table);
        if let Some(base) = frames.get("fixed-L3") {
            for (name, v) in &frames {
                if name != "fixed-L3" {
                    let d: Vec<f64> = v.iter().zip(base).map(|(a, b)| a - b).collect();
                    let m = d.iter().sum::<f64>() / d.len() as f64;
                    let se = (d.iter().map(|x| (x - m).powi(2)).sum::<f64>() / (d.len() - 1) as f64).sqrt() / (d.len() as f64).sqrt();
                    println!("   {name} - fixed-L3: {m:+.0} +/- {se:.0} bit/s per frame (paired, 1 s.e.)");
                }
            }
        }
    }
    Ok(())
}

/// Two-way adaptive file transfer (simulator or over the air), optionally with changing noise.
pub fn duplex(hosts: Option<&Hosts>, file_a: &str, file_b: &str, noise: bool, rounds_per_phase: usize, max_rounds: usize) -> anyhow::Result<()> {
    let phy = Phy::new(PhyConfig::default());
    let fs = phy.cfg.fs as f64;
    let (da, db) = (std::fs::read(file_a)?, std::fs::read(file_b)?);
    let mut a = Node::new(&phy, FileSender::new(da.clone(), 1), 1);
    let mut b = Node::new(&phy, FileSender::new(db.clone(), 2), 2);
    let (mut rx_a, mut rx_b) = (FileReceiver::default(), FileReceiver::default());
    let phases = scenario::phases();
    let phase = |rnd: usize| -> &[Interferer] { if noise { &phases[(rnd / rounds_per_phase).min(phases.len() - 1)].1 } else { &[] } };
    let mut rng = StdRng::seed_from_u64(0);
    let mut sims = (AcousticChannel::new(CHANNELS[0], 1)?, AcousticChannel::new(CHANNELS[1], 2)?);
    let cal = match (hosts, noise) {
        (Some(h), true) => Some((calibrate_noise(h, &h.a, &phy, &mut rng)?, calibrate_noise(h, &h.b, &phy, &mut rng)?)),
        _ => None,
    };
    let t0 = std::time::Instant::now();
    let mut rounds = 0;
    for rnd in 0..max_rounds {
        rounds = rnd + 1;
        let its = phase(rnd).to_vec();
        let wave = a.make_frame(&phy);
        let rec = match hosts {
            Some(h) => air::transmit(h, &h.a, &wave, cal.and_then(|c| broadcast_noise(&its, &phy, c.0, &mut rng)).as_deref())?,
            None => sims.0.apply(&wave, fs, &its),
        };
        b.on_receive(&phy, &rec, |blk| rx_b.put(blk));
        let wave = b.make_frame(&phy);
        let rec = match hosts {
            Some(h) => air::transmit(h, &h.b, &wave, cal.and_then(|c| broadcast_noise(&its, &phy, c.1, &mut rng)).as_deref())?,
            None => sims.1.apply(&wave, fs, &its),
        };
        a.on_receive(&phy, &rec, |blk| rx_a.put(blk));
        let map = |l: &[u8]| l.iter().map(|v| v.to_string()).collect::<String>();
        println!("round {rnd:2}: A->B {:6.1}% (A map {})  B->A {:6.1}%", 100.0 * rx_b.progress(), &map(&a.out_levels)[..40.min(a.out_levels.len())], 100.0 * rx_a.progress());
        if a.traffic.done() && b.traffic.done() {
            break;
        }
    }
    for (label, sent, got) in [("A->B", &da, &rx_b.buf), ("B->A", &db, &rx_a.buf)] {
        println!("{label}: {} bytes, {}", sent.len(), if sent == got { "IDENTICAL" } else { "MISMATCH" });
    }
    let airtime = (a.stats.frames + b.stats.frames) as f64 * phy.cfg.frame_s();
    println!(
        "{rounds} rounds, {airtime:.0}s airtime ({:.0}s wall): {:.0} bit/s goodput, both directions combined",
        t0.elapsed().as_secs_f64(),
        8.0 * (da.len() + db.len()) as f64 / airtime
    );
    Ok(())
}

/// Test frames for other decoders: DIR/frame_<name>.wav + .json (header and payloads).
pub fn interop_gen(dir: &str) -> anyhow::Result<()> {
    std::fs::create_dir_all(dir)?;
    let phy = Phy::new(PhyConfig::default());
    let n = phy.cfg.n_tiles();
    let mut rng = StdRng::seed_from_u64(99);
    for (name, levels) in [
        ("mixed", (0..n).map(|k| (k % 4) as u8).collect::<Vec<u8>>()),
        ("l3", vec![3u8; n]),
        ("l1", vec![1u8; n]),
    ] {
        let h = Header {
            seq: 4242,
            levels: levels.clone(),
            fb_seq: 4241,
            fb_ok: (0..n).map(|k| k % 3 == 0).collect(),
            fb_levels: (0..n).map(|k| ((k + 1) % 4) as u8).collect(),
        };
        let (wave, pl) = build_frame(&phy, &h, &levels, random_bytes(&mut rng));
        let x = [vec![0.0; 4800], wave, vec![0.0; 4800]].concat();
        crate::audio::write_wav(&format!("{dir}/frame_{name}.wav"), &x)?;
        let meta = serde_json::json!({
            "seq": h.seq, "fb_seq": h.fb_seq, "levels": levels, "fb_levels": h.fb_levels,
            "fb_ok": h.fb_ok.iter().map(|&o| o as u8).collect::<Vec<u8>>(),
            "payloads": pl.iter().map(|p| p.iter().map(|b| format!("{b:02x}")).collect::<String>()).collect::<Vec<_>>(),
        });
        std::fs::write(format!("{dir}/frame_{name}.json"), meta.to_string())?;
    }
    println!("wrote Rust frames to {dir}");
    Ok(())
}
