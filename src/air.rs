//! Over-the-air I/O from a control machine: play a WAV on one host while recording on the other
//! over ssh (optionally with the receiver broadcasting interference from its own speaker).
//! Hosts are ssh aliases with a PipeWire desktop session.

use crate::audio::{read_wav, write_wav, FS};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

const ENV: &str = "export XDG_RUNTIME_DIR=/run/user/$(id -u);";

#[derive(Clone)]
pub struct Hosts {
    pub a: String,
    pub b: String,
}

impl Default for Hosts {
    fn default() -> Self {
        Self { a: "dell-g5".into(), b: "e540".into() }
    }
}

impl Hosts {
    pub fn peer(&self, h: &str) -> String {
        if h == self.a { self.b.clone() } else { self.a.clone() }
    }
    pub fn both(&self) -> [String; 2] {
        [self.a.clone(), self.b.clone()]
    }
}

fn ssh(host: &str, cmd: &str) -> anyhow::Result<Child> {
    Ok(Command::new("ssh").arg(host).arg(format!("{ENV}{cmd}")).stdout(Stdio::null()).stderr(Stdio::null()).spawn()?)
}

fn scp(from: &str, to: &str) -> anyhow::Result<()> {
    anyhow::ensure!(Command::new("scp").args(["-q", from, to]).status()?.success(), "scp {from} {to}");
    Ok(())
}

fn tmp(name: &str) -> String {
    format!("{}/dos-air-{}-{name}", std::env::temp_dir().display(), std::process::id())
}

/// Play `signal` on `tx`, record on its peer; the receiver optionally plays `interferer`.
pub fn transmit(hosts: &Hosts, tx: &str, signal: &[f64], interferer: Option<&[f64]>) -> anyhow::Result<Vec<f64>> {
    let rx = hosts.peer(tx);
    let (lead, tail) = (0.8, 0.8);
    let dur = signal.len() as f64 / FS as f64 + lead + tail + 0.6;
    write_wav(&tmp("tx.wav"), signal)?;
    scp(&tmp("tx.wav"), &format!("{tx}:/tmp/dos_tx.wav"))?;
    let mut noise = String::new();
    if let Some(n) = interferer {
        write_wav(&tmp("noise.wav"), n)?;
        scp(&tmp("noise.wav"), &format!("{rx}:/tmp/dos_noise.wav"))?;
        noise = "pw-play /tmp/dos_noise.wav & ".into();
    }
    let mut rec = ssh(&rx, &format!("{noise}timeout {dur:.1} pw-record --rate 48000 --channels 1 --format s16 /tmp/dos_rx.wav; wait"))?;
    std::thread::sleep(Duration::from_secs_f64(lead + 0.6));
    ssh(tx, "pw-play /tmp/dos_tx.wav")?.wait()?;
    rec.wait()?;
    scp(&format!("{rx}:/tmp/dos_rx.wav"), &tmp("rx.wav"))?;
    read_wav(&tmp("rx.wav"))
}

pub fn record_silence(host: &str, seconds: f64) -> anyhow::Result<Vec<f64>> {
    ssh(host, &format!("timeout {seconds:.1} pw-record --rate 48000 --channels 1 --format s16 /tmp/dos_sil.wav"))?.wait()?;
    scp(&format!("{host}:/tmp/dos_sil.wav"), &tmp("sil.wav"))?;
    read_wav(&tmp("sil.wav"))
}
