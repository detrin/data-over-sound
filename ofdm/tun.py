"""Minimal Linux TUN device access (layer-3, no packet-info header)."""
import fcntl
import os
import struct

TUNSETIFF = 0x400454CA
IFF_TUN, IFF_NO_PI = 0x0001, 0x1000


class Tun:
    """Attach to an existing (persistent, user-owned) TUN device, e.g. created with
    `ip tuntap add dev ofdm0 mode tun user $USER`, so the daemon needs no root."""

    def __init__(self, name: str = "ofdm0"):
        self.name = name
        self.fd = os.open("/dev/net/tun", os.O_RDWR)
        fcntl.ioctl(self.fd, TUNSETIFF, struct.pack("16sH", name.encode(), IFF_TUN | IFF_NO_PI))

    def read(self, mtu: int = 2048) -> bytes:
        return os.read(self.fd, mtu)

    def write(self, pkt: bytes):
        os.write(self.fd, pkt)
