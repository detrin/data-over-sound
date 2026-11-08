//! The adaptive OFDM modem as a network card (TUN device, e.g. ofdm0).
//!
//! Medium access is strict turn-taking: the leader sends a frame and listens; the follower answers
//! every frame it hears. If the leader hears nothing within a timeout it sends again; with no
//! traffic either way it slows to one exchange per `idle_gap` (new packets wake it). Each node
//! ignores what it recorded while transmitting, plus a reverb tail.

use crate::audio::{self, Recorder, FS};
use crate::controllers::ThompsonSw;
use crate::dsp;
use crate::link::receive_frame;
use crate::node::Node;
use crate::phy::{Phy, PhyConfig};
use crate::traffic::{PacketQueue, PacketReassembly, Traffic};
use crate::tun;
use std::fs::File;
use std::process::Command;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

const TAIL_S: f64 = 0.7;
const DECODE_MARGIN_S: f64 = 0.4;
const FALSE_ALARM_SNR_DB: f64 = 3.0;

#[derive(Clone, Copy, PartialEq, Eq, Debug, clap::ValueEnum)]
pub enum Role {
    Leader,
    Follower,
}

/// The packet queue shared between the TUN reader thread and the modem loop.
#[derive(Clone)]
struct Shared(Arc<(Mutex<PacketQueue>, Condvar)>);

impl Traffic for Shared {
    fn fill(&mut self, seq: u16, caps: &[usize]) -> Vec<Vec<u8>> {
        self.0 .0.lock().unwrap().fill(seq, caps)
    }
    fn ack(&mut self, seq: u16, ok: &[bool]) {
        self.0 .0.lock().unwrap().ack(seq, ok)
    }
    fn requeue_before(&mut self, seq: u16) {
        self.0 .0.lock().unwrap().requeue_before(seq)
    }
}

struct Modem {
    phy: Phy,
    rec: Recorder,
    tun_w: File,
    queue: Shared,
    reasm: PacketReassembly,
    node: Node<Shared>,
    role: Role,
}

pub fn hms() -> String {
    unsafe {
        let t = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&t, &mut tm);
        format!("{:02}:{:02}:{:02}", tm.tm_hour, tm.tm_min, tm.tm_sec)
    }
}

fn log(role: Role, msg: &str) {
    println!("{} [{}] {msg}", hms(), match role { Role::Leader => "leader", Role::Follower => "follower" });
}

impl Modem {
    fn transmit(&mut self) -> anyhow::Result<usize> {
        let wave = self.node.make_frame(&self.phy);
        audio::play(&wave)?;
        Ok(self.rec.now() + (TAIL_S * FS as f64) as usize)
    }

    fn is_false_alarm(&self, seg: &[f64]) -> bool {
        let r = receive_frame(&self.phy, seg, None, None);
        r.header.is_none() && !(dsp::median(&r.tile_snr_db) > FALSE_ALARM_SNR_DB)
    }

    /// (segment holding one peer frame, new listen index) or (None, listen index) after timeout.
    fn wait_frame(&self, mut listen_from: usize, timeout: Duration) -> (Option<Vec<f64>>, usize) {
        let frame = self.phy.cfg.frame_len();
        let sync = self.phy.sync.len();
        let t0 = Instant::now();
        while t0.elapsed() < timeout {
            std::thread::sleep(Duration::from_millis(400));
            let mut x = self.rec.since(listen_from);
            if x.len() > 3 * frame {
                listen_from += x.len() - 2 * frame; // nothing in a long stretch: slide on
                x = self.rec.since(listen_from);
            }
            if x.len() < frame / 2 {
                continue;
            }
            let Some(st) = self.phy.find_sync(&x) else { continue };
            let end = st - sync + frame + (DECODE_MARGIN_S * FS as f64) as usize;
            if x.len() < end {
                continue; // frame still arriving
            }
            let begin = (st - sync).saturating_sub(FS / 10);
            if self.is_false_alarm(&x[begin..end]) {
                log(self.role, "ignored a sync hit on noise");
                listen_from += st;
                continue;
            }
            return (Some(x[begin..end].to_vec()), listen_from + end);
        }
        (None, listen_from)
    }

    fn receive(&mut self, seg: &[f64]) -> bool {
        let t = Instant::now();
        let mut out = vec![];
        let reasm = &mut self.reasm;
        let r = self.node.on_receive(&self.phy, seg, |b| out.extend(reasm.put(b)));
        for p in &out {
            let _ = tun::write_packet(&mut self.tun_w, p);
        }
        let mut lv = [0usize; 4];
        if let Some(h) = &r.header {
            h.levels.iter().for_each(|&l| lv[l as usize] += 1);
        }
        let q = self.queue.0 .0.lock().unwrap().pending_bytes();
        log(
            self.role,
            &format!(
                "rx seq {:>5} {} tiles ok {:2}/{} levels p/1/2/3 {}/{}/{}/{} payload {:5} B snr med {:4.1} dB | tx queue {} B | delivered {} pkts | decode {:.1}s",
                r.header.as_ref().map_or("----".into(), |h| h.seq.to_string()),
                if r.header.is_some() { "hdr ok  " } else { "HDR LOST" },
                r.tile_ok.iter().filter(|&&o| o).count(),
                self.phy.cfg.n_tiles(),
                lv[0], lv[1], lv[2], lv[3],
                r.payload_bytes,
                dsp::median(&r.tile_snr_db),
                q,
                self.reasm.delivered,
                t.elapsed().as_secs_f64()
            ),
        );
        r.header.is_some() && r.payload_bytes > 0 && !out.is_empty()
    }
}

/// Run the daemon in the foreground.
pub fn run(role: Role, dev: &str, idle_gap: f64) -> anyhow::Result<()> {
    let phy = Phy::new(PhyConfig::default());
    log(role, &format!("{} tiles {:.0}-{:.0} Hz, frame {:.1}s on {dev}", phy.cfg.n_tiles(), phy.cfg.f_lo, phy.cfg.f_hi, phy.cfg.frame_s()));
    let tun_r = tun::open(dev)?;
    let tun_w = tun_r.try_clone()?;
    let queue = Shared(Arc::new((Mutex::new(PacketQueue::new(1)), Condvar::new())));
    let q2 = queue.clone();
    std::thread::spawn(move || {
        let mut f = tun_r;
        while let Ok(p) = tun::read_packet(&mut f) {
            let (m, cv) = &*q2.0;
            m.lock().unwrap().push(p);
            cv.notify_all();
        }
    });
    // packets span several tiles and need all of them: a risk-averse controller (see ThompsonSw::risk)
    let ctrl = ThompsonSw::for_packets(&phy.cfg, match role { Role::Leader => 1, Role::Follower => 2 });
    let node = Node::with_controller(&phy, queue.clone(), Box::new(ctrl));
    let mut m = Modem { phy, rec: Recorder::start(60.0)?, tun_w, queue, reasm: PacketReassembly::default(), node, role };
    let timeout = Duration::from_secs_f64(m.phy.cfg.frame_s() + 8.0);
    match role {
        Role::Leader => loop {
            let listen = m.transmit()?;
            let (seg, _) = m.wait_frame(listen, timeout);
            let Some(seg) = seg else {
                m.node.on_lost();
                log(role, "no answer from follower (timeout)");
                continue;
            };
            let got_data = m.receive(&seg);
            let (mx, cv) = &*m.queue.0;
            let q = mx.lock().unwrap();
            if q.pending_bytes() == 0 && !got_data {
                let _ = cv.wait_timeout(q, Duration::from_secs_f64(idle_gap)).unwrap();
            }
        },
        Role::Follower => {
            let mut listen = m.rec.now();
            loop {
                let (seg, next) = m.wait_frame(listen, Duration::from_secs(3600));
                listen = next;
                if let Some(seg) = seg {
                    m.receive(&seg);
                    listen = m.transmit()?;
                }
            }
        }
    }
}

fn sudo(args: &[&str]) -> anyhow::Result<()> {
    let st = Command::new("sudo").args(args).status()?;
    anyhow::ensure!(st.success(), "sudo {} failed", args.join(" "));
    Ok(())
}

fn whoami() -> String {
    std::env::var("USER").unwrap_or_else(|_| String::from_utf8_lossy(&Command::new("id").arg("-un").output().map(|o| o.stdout).unwrap_or_default()).trim().to_string())
}

/// Create the user-owned TUN device, address, MTU and a TCP-friendly route (uses sudo).
pub fn up(dev: &str, cidr: &str, mtu: u32, rto_min: &str) -> anyhow::Result<()> {
    let _ = Command::new("sudo").args(["ip", "tuntap", "add", "dev", dev, "mode", "tun", "user", &whoami()]).stderr(std::process::Stdio::null()).status();
    sudo(&["ip", "addr", "replace", cidr, "dev", dev])?;
    sudo(&["ip", "link", "set", dev, "mtu", &mtu.to_string(), "up"])?;
    let out = Command::new("ip").args(["-4", "route", "show", "dev", dev, "proto", "kernel"]).output()?;
    if let Some(net) = String::from_utf8_lossy(&out.stdout).split_whitespace().next() {
        // one exchange takes ~15-20 s: keep TCP from retransmitting every second
        sudo(&["ip", "route", "replace", net, "dev", dev, "proto", "kernel", "scope", "link", "rto_min", rto_min])?;
    }
    let _ = Command::new("ip").args(["-br", "addr", "show", "dev", dev]).status();
    Ok(())
}

pub fn down(dev: &str) -> anyhow::Result<()> {
    stop(dev);
    let _ = Command::new("sudo").args(["ip", "link", "del", dev]).status();
    Ok(())
}

fn pidfile(dev: &str) -> String {
    format!("/tmp/dos-net-{dev}.pid")
}

/// Start the daemon in the background (detached), logging to `log`.
pub fn start(role: Role, dev: &str, idle_gap: f64, logfile: &str) -> anyhow::Result<()> {
    stop(dev);
    let exe = std::env::current_exe()?;
    let log = File::create(logfile)?;
    let mut cmd = Command::new(exe);
    cmd.args(["net", "run", "--role", match role { Role::Leader => "leader", Role::Follower => "follower" }, "--dev", dev, "--idle-gap", &idle_gap.to_string()])
        .stdout(log.try_clone()?)
        .stderr(log)
        .stdin(std::process::Stdio::null());
    if std::env::var("XDG_RUNTIME_DIR").is_err() {
        cmd.env("XDG_RUNTIME_DIR", format!("/run/user/{}", unsafe { libc::getuid() }));
    }
    #[cfg(unix)]
    unsafe {
        use std::os::unix::process::CommandExt;
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    let child = cmd.spawn()?;
    std::fs::write(pidfile(dev), child.id().to_string())?;
    std::thread::sleep(Duration::from_secs(2));
    print!("{}", std::fs::read_to_string(logfile).unwrap_or_default());
    Ok(())
}

pub fn stop(dev: &str) {
    if let Ok(pid) = std::fs::read_to_string(pidfile(dev)) {
        if let Ok(pid) = pid.trim().parse::<i32>() {
            if unsafe { libc::kill(pid, libc::SIGTERM) } == 0 {
                println!("stopped");
            }
        }
    }
    let _ = std::fs::remove_file(pidfile(dev));
}

pub fn status(dev: &str, logfile: &str) {
    let _ = Command::new("ip").args(["-br", "addr", "show", "dev", dev]).status();
    let running = std::fs::read_to_string(pidfile(dev))
        .ok()
        .and_then(|p| p.trim().parse::<i32>().ok())
        .filter(|&p| unsafe { libc::kill(p, 0) } == 0);
    match running {
        Some(p) => println!("daemon running (pid {p})"),
        None => println!("daemon not running"),
    }
    let log = std::fs::read_to_string(logfile).unwrap_or_default();
    for l in log.lines().rev().take(5).collect::<Vec<_>>().into_iter().rev() {
        println!("{l}");
    }
}
