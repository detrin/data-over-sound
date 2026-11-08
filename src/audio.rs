//! Real-time audio through PipeWire's command-line tools: one long-running `pw-record` streaming
//! raw s16 mono into a buffer indexed by absolute sample number, and one `pw-play` per frame
//! (through a temporary WAV file: newer pw-play versions refuse raw audio on stdin).

use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};

pub const FS: usize = 48000;

struct Buf {
    data: Vec<f32>,
    start: usize,
}

pub struct Recorder {
    child: Child,
    buf: Arc<Mutex<Buf>>,
}

impl Recorder {
    pub fn start(keep_s: f64) -> anyhow::Result<Self> {
        let mut child = Command::new("pw-record")
            .args(["--rate", "48000", "--channels", "1", "--format", "s16", "-"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let mut out = child.stdout.take().unwrap();
        let buf = Arc::new(Mutex::new(Buf { data: Vec::new(), start: 0 }));
        let keep = (keep_s * FS as f64) as usize;
        let b = buf.clone();
        std::thread::spawn(move || {
            let mut raw = vec![0u8; FS / 10 * 2];
            let mut carry: Option<u8> = None;
            loop {
                let n = match out.read(&mut raw) {
                    Ok(0) | Err(_) => return,
                    Ok(n) => n,
                };
                let mut bytes: Vec<u8> = carry.take().into_iter().collect();
                bytes.extend_from_slice(&raw[..n]);
                if bytes.len() % 2 == 1 {
                    carry = bytes.pop();
                }
                let mut g = b.lock().unwrap();
                g.data.extend(bytes.chunks(2).map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0));
                if g.data.len() > keep {
                    let drop = g.data.len() - keep;
                    g.data.drain(..drop);
                    g.start += drop;
                }
            }
        });
        Ok(Self { child, buf })
    }

    /// Absolute index one past the newest sample.
    pub fn now(&self) -> usize {
        let g = self.buf.lock().unwrap();
        g.start + g.data.len()
    }

    /// Samples from absolute index `idx` to now.
    pub fn since(&self, idx: usize) -> Vec<f64> {
        let g = self.buf.lock().unwrap();
        let o = idx.saturating_sub(g.start).min(g.data.len());
        g.data[o..].iter().map(|&v| v as f64).collect()
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

pub fn write_wav(path: &str, x: &[f64]) -> anyhow::Result<()> {
    let spec = hound::WavSpec { channels: 1, sample_rate: FS as u32, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
    let mut w = hound::WavWriter::create(path, spec)?;
    for &v in x {
        w.write_sample((v.clamp(-1.0, 1.0) * 32767.0) as i16)?;
    }
    w.finalize()?;
    Ok(())
}

pub fn read_wav(path: &str) -> anyhow::Result<Vec<f64>> {
    let mut r = hound::WavReader::open(path)?;
    let ch = r.spec().channels as usize;
    let s: Vec<f64> = r.samples::<i16>().map(|v| v.map(|v| v as f64 / 32768.0)).collect::<Result<_, _>>()?;
    Ok(if ch == 1 { s } else { s.chunks(ch).map(|c| c.iter().sum::<f64>() / ch as f64).collect() })
}

/// Blocking: returns when PipeWire has played the whole buffer.
pub fn play(x: &[f64]) -> anyhow::Result<()> {
    let dir = if std::path::Path::new("/dev/shm").is_dir() { "/dev/shm" } else { "/tmp" };
    let path = format!("{dir}/dos-play-{}.wav", std::process::id());
    write_wav(&path, x)?;
    let st = Command::new("pw-play").arg(&path).stdout(Stdio::null()).stderr(Stdio::null()).status();
    let _ = std::fs::remove_file(&path);
    anyhow::ensure!(st?.success(), "pw-play failed");
    Ok(())
}
