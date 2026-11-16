use dos::channel::ChannelData;
use dos_wasm::classic::{modulate, AfskConfig, Classic, ClassicState, UartRx};
use dos_wasm::sim::Air;
use std::process::Command;

const TEXT: &[u8] = b"BEGIN_DATA\nSGVsbG8sIHNvdW5kIQ==\nEND_DATA\n";

fn minimodem() -> Option<String> {
    ["minimodem", "/opt/homebrew/bin/minimodem", "/usr/bin/minimodem"]
        .into_iter()
        .find(|m| Command::new(m).arg("--version").output().is_ok())
        .map(String::from)
}

fn tmp(name: &str) -> String {
    let d = std::env::temp_dir().join("dos-wasm-classic");
    std::fs::create_dir_all(&d).unwrap();
    d.join(name).to_string_lossy().into_owned()
}

fn write_wav(path: &str, x: &[f32]) {
    let spec = hound::WavSpec { channels: 1, sample_rate: 48_000, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
    let mut w = hound::WavWriter::create(path, spec).unwrap();
    for &v in x {
        w.write_sample((v.clamp(-1.0, 1.0) * 32767.0) as i16).unwrap();
    }
    w.finalize().unwrap();
}

fn read_wav(path: &str) -> Vec<f32> {
    hound::WavReader::open(path).unwrap().samples::<i16>().map(|s| s.unwrap() as f32 / 32768.0).collect()
}

#[test]
fn own_modem_round_trip() {
    let cfg = AfskConfig::default();
    let mut rx = UartRx::new(&cfg);
    let mut sig = vec![0.0; 3000];
    sig.extend(modulate(&cfg, TEXT));
    sig.extend(vec![0.0; 3000]);
    let got: Vec<u8> = sig.chunks(960).flat_map(|c| rx.push(c)).collect();
    assert_eq!(got, TEXT);
}

#[test]
fn minimodem_decodes_our_audio() {
    let Some(mm) = minimodem() else { return eprintln!("minimodem not installed, skipped") };
    let path = tmp("ours.wav");
    let mut sig = vec![0.0; 4800];
    sig.extend(modulate(&AfskConfig::default(), TEXT));
    sig.extend(vec![0.0; 4800]);
    write_wav(&path, &sig);
    let out = Command::new(mm).args(["--rx", "--quiet", "--mark", "2300", "--space", "1900", "-f", &path, "100"]).output().unwrap();
    assert_eq!(out.stdout, TEXT, "minimodem heard {:?}", String::from_utf8_lossy(&out.stdout));
}

#[test]
fn we_decode_minimodem_audio() {
    let Some(mm) = minimodem() else { return eprintln!("minimodem not installed, skipped") };
    let path = tmp("theirs.wav");
    let mut child = Command::new(mm)
        .args(["--tx", "--quiet", "--mark", "2300", "--space", "1900", "-R", "48000", "-f", &path, "100"])
        .stdin(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    use std::io::Write;
    child.stdin.take().unwrap().write_all(TEXT).unwrap();
    child.wait().unwrap();
    let mut x = vec![0.0; 2400];
    x.extend(read_wav(&path));
    x.extend(vec![0.0; 2400]);
    let mut rx = UartRx::new(&AfskConfig::default());
    let got: Vec<u8> = x.chunks(960).flat_map(|c| rx.push(c)).collect();
    assert_eq!(got, TEXT, "we heard {:?}", String::from_utf8_lossy(&got));
}

#[test]
fn stop_and_wait_across_the_measured_room() {
    let ab = ChannelData::load("dell-g5_to_e540").unwrap();
    let ba = ChannelData::load("e540_to_dell-g5").unwrap();
    let mut air = Air::new(ab, ba, 48_000, 9);
    let data: Vec<u8> = (0..150u32).map(|i| (i * 7 % 256) as u8).collect();
    let cfg = AfskConfig::default();
    let mut st = [Classic::sender(cfg, &data), Classic::receiver(cfg)];
    let mut playing = [0usize; 2];
    let (mut t, block) = (0usize, 960usize);
    let mut got = None;
    while t < 48_000 * 400 {
        let now = t as f64 / 48.0;
        for i in 0..2 {
            let heard = air.hear(i, block);
            st[i].push_audio(&heard, now);
            if playing[i] == 0 {
                if let Some(w) = st[i].poll(now) {
                    air.transmit(i, &w);
                    playing[i] = w.len();
                }
            }
        }
        t += block;
        for i in 0..2 {
            if playing[i] > 0 {
                playing[i] = playing[i].saturating_sub(block);
                if playing[i] == 0 {
                    st[i].tx_done(t as f64 / 48.0);
                }
            }
        }
        got = got.or_else(|| st[1].take_completed());
        if got.is_some() && st[0].status().state == ClassicState::Done {
            break;
        }
    }
    eprintln!("sender log tail: {:?}", st[0].status().log.iter().rev().take(4).collect::<Vec<_>>());
    assert_eq!(got.as_deref(), Some(&data[..]), "receiver should rebuild the file");
    assert_eq!(st[0].status().state, ClassicState::Done);
    eprintln!("150 B by stop-and-wait in {:.0} s of simulated air", t as f64 / 48_000.0);
}
