//! Minimal Linux TUN access (layer 3, no packet-info header). Attaches to an existing persistent
//! device owned by the user (`dos net up` creates it), so the daemon needs no root.

use std::fs::File;
use std::io::{Read, Write};

#[cfg(target_os = "linux")]
pub fn open(name: &str) -> anyhow::Result<File> {
    use std::fs::OpenOptions;
    use std::os::fd::AsRawFd;
    const TUNSETIFF: libc::c_ulong = 0x400454CA;
    const IFF_TUN: libc::c_short = 0x0001;
    const IFF_NO_PI: libc::c_short = 0x1000;
    #[repr(C)]
    struct IfReq {
        name: [u8; 16],
        flags: libc::c_short,
        _pad: [u8; 22],
    }
    let f = OpenOptions::new().read(true).write(true).open("/dev/net/tun")?;
    let mut req = IfReq { name: [0; 16], flags: IFF_TUN | IFF_NO_PI, _pad: [0; 22] };
    anyhow::ensure!(name.len() < 16, "interface name too long");
    req.name[..name.len()].copy_from_slice(name.as_bytes());
    let rc = unsafe { libc::ioctl(f.as_raw_fd(), TUNSETIFF as _, &mut req) };
    anyhow::ensure!(rc >= 0, "TUNSETIFF {name}: {}", std::io::Error::last_os_error());
    Ok(f)
}

#[cfg(not(target_os = "linux"))]
pub fn open(_name: &str) -> anyhow::Result<File> {
    anyhow::bail!("TUN devices are only supported on Linux")
}

pub fn read_packet(f: &mut File) -> std::io::Result<Vec<u8>> {
    let mut buf = vec![0u8; 2048];
    let n = f.read(&mut buf)?;
    buf.truncate(n);
    Ok(buf)
}

pub fn write_packet(f: &mut File, pkt: &[u8]) -> std::io::Result<()> {
    f.write_all(pkt)
}
