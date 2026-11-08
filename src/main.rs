//! `dos`: data over sound. Network card, file-transfer modem and research tools in one binary.

use clap::{Parser, Subcommand};
use dos::air::Hosts;
use dos::{experiments, legacy, net};
use std::time::Duration;

#[derive(Parser)]
#[command(name = "dos", version, about = "Data over sound: adaptive OFDM acoustic modem and network card")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(clap::Args, Clone)]
struct HostArgs {
    /// ssh aliases of the two machines (control-machine experiments)
    #[arg(long, default_value = "dell-g5,e540")]
    hosts: String,
}

impl HostArgs {
    fn hosts(&self) -> Hosts {
        let mut it = self.hosts.split(',');
        Hosts { a: it.next().unwrap_or("dell-g5").into(), b: it.next().unwrap_or("e540").into() }
    }
}

#[derive(Subcommand)]
enum Cmd {
    /// Network card over sound (TUN device)
    Net {
        #[command(subcommand)]
        cmd: NetCmd,
        #[arg(long, default_value = "ofdm0", global = true)]
        dev: String,
        #[arg(long, default_value = "/tmp/dos-net.log", global = true)]
        log: String,
    },
    /// File transfer with the original minimodem (100 baud MFSK) modem
    Modem {
        #[command(subcommand)]
        cmd: ModemCmd,
        #[arg(long, default_value = "minimodem", global = true)]
        minimodem: String,
        #[arg(long, default_value_t = 100.0, global = true)]
        baud: f64,
        #[arg(long, default_value_t = 2300.0, global = true)]
        mark: f64,
        #[arg(long, default_value_t = 1900.0, global = true)]
        space: f64,
        #[arg(long, global = true)]
        volume: Option<f64>,
        /// payload bytes per packet
        #[arg(long, default_value_t = 64, global = true)]
        payload_size: usize,
        /// Reed-Solomon parity bytes per packet (0 = off)
        #[arg(long, default_value_t = 16, global = true)]
        fec: usize,
        #[arg(long, default_value_t = 5, global = true)]
        max_retries: u32,
        /// ACK timeout in s (default: computed from the airtime)
        #[arg(long, global = true)]
        timeout: Option<f64>,
        #[arg(long, default_value_t = 1.0, global = true)]
        turnaround: f64,
    },
    /// Loopback and simulated-channel PHY check at fixed levels
    Check,
    /// Measure the room (impulse response + noise) in both directions (control machine, ssh)
    Measure(HostArgs),
    /// Re-fit the SNR -> success model in the simulator
    Calibrate {
        #[arg(long, default_value_t = 10)]
        frames: usize,
        #[arg(long, default_value = "data/snr_model.json")]
        out: String,
    },
    /// Compare link-adaptation strategies in the simulator under changing noise
    Simulate {
        #[arg(long, default_value_t = 8)]
        frames_per_phase: usize,
        #[arg(long, default_value_t = 2)]
        seeds: u64,
        #[arg(long)]
        finalists: bool,
    },
    /// Compare strategies over the air with broadcast noise (control machine, ssh)
    Compare {
        #[command(flatten)]
        hosts: HostArgs,
        #[arg(long, default_value_t = 4)]
        frames_per_phase: usize,
        #[arg(long)]
        finalists: bool,
    },
    /// Two-way adaptive file transfer, simulated or over the air
    Duplex {
        #[command(flatten)]
        hosts: HostArgs,
        #[arg(long)]
        air: bool,
        /// changing noise (scenario phases), broadcast by the receiver over the air
        #[arg(long)]
        noise: bool,
        #[arg(long, default_value = "Feels_good_man.jpg")]
        file_a: String,
        #[arg(long, default_value = "README.md")]
        file_b: String,
        #[arg(long, default_value_t = 3)]
        rounds_per_phase: usize,
        #[arg(long, default_value_t = 40)]
        max_rounds: usize,
    },
    /// Write test frames (WAV + JSON with header and payloads) for other decoders
    InteropGen { dir: String },
}

#[derive(Subcommand)]
enum NetCmd {
    /// Create the user-owned TUN device, address, MTU and route (sudo)
    Up {
        cidr: String,
        /// small packets span few tiles, so they still complete when the room gets noisy
        #[arg(long, default_value_t = 576)]
        mtu: u32,
        #[arg(long, default_value = "5s")]
        rto_min: String,
    },
    /// Stop the daemon and delete the interface (sudo)
    Down,
    /// Run the daemon in the foreground
    Run {
        #[arg(long, value_enum)]
        role: net::Role,
        #[arg(long, default_value_t = 4.0)]
        idle_gap: f64,
    },
    /// Run the daemon in the background
    Start {
        #[arg(value_enum)]
        role: net::Role,
        #[arg(long, default_value_t = 4.0)]
        idle_gap: f64,
    },
    Stop,
    Status,
}

#[derive(Subcommand)]
enum ModemCmd {
    /// Send a file (start the receiver first)
    Send { file: String },
    /// Receive a file
    Recv {
        out: String,
        #[arg(long, default_value_t = 900.0)]
        max_wait: f64,
    },
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Net { cmd, dev, log } => match cmd {
            NetCmd::Up { cidr, mtu, rto_min } => net::up(&dev, &cidr, mtu, &rto_min)?,
            NetCmd::Down => net::down(&dev)?,
            NetCmd::Run { role, idle_gap } => net::run(role, &dev, idle_gap)?,
            NetCmd::Start { role, idle_gap } => net::start(role, &dev, idle_gap, &log)?,
            NetCmd::Stop => net::stop(&dev),
            NetCmd::Status => net::status(&dev, &log),
        },
        Cmd::Modem { cmd, minimodem, baud, mark, space, volume, payload_size, fec, max_retries, timeout, turnaround } => {
            let a = legacy::TransferArgs {
                modem: legacy::ModemArgs { minimodem, baud, mark: Some(mark), space: Some(space), volume, turnaround },
                payload_size,
                fec,
                max_retries,
                timeout,
            };
            match cmd {
                ModemCmd::Send { file } => {
                    let data = std::fs::read(&file)?;
                    let t = std::time::Instant::now();
                    let ok = legacy::send(&a, &data);
                    let s = t.elapsed().as_secs_f64();
                    println!("{} {} bytes in {s:.0}s ({:.1} bit/s)", if ok { "Sent" } else { "FAILED to send" }, data.len(), 8.0 * data.len() as f64 / s);
                    anyhow::ensure!(ok, "transfer failed");
                }
                ModemCmd::Recv { out, max_wait } => {
                    let (data, complete) = legacy::receive(&a, Duration::from_secs_f64(max_wait));
                    std::fs::write(&out, &data)?;
                    println!("Wrote {} bytes to {out}{}", data.len(), if complete { "" } else { " (INCOMPLETE)" });
                    anyhow::ensure!(complete, "incomplete");
                }
            }
        }
        Cmd::Check => experiments::check()?,
        Cmd::Measure(h) => experiments::measure(&h.hosts())?,
        Cmd::Calibrate { frames, out } => experiments::calibrate(frames, &out)?,
        Cmd::Simulate { frames_per_phase, seeds, finalists } => experiments::simulate(frames_per_phase, seeds, finalists)?,
        Cmd::Compare { hosts, frames_per_phase, finalists } => experiments::compare_air(&hosts.hosts(), frames_per_phase, finalists)?,
        Cmd::Duplex { hosts, air, noise, file_a, file_b, rounds_per_phase, max_rounds } => {
            let h = hosts.hosts();
            experiments::duplex(air.then_some(&h), &file_a, &file_b, noise, rounds_per_phase, max_rounds)?
        }
        Cmd::InteropGen { dir } => experiments::interop_gen(&dir)?,
    }
    Ok(())
}
