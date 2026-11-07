#!/usr/bin/env python3
"""
Sound Modem - Main Entry Point

Wires packet_framer (CRC + Reed-Solomon + COBS + Stop-and-Wait ARQ) to sound_packet_bridge
(minimodem over the air) and transfers one file:

    python main.py [options] rx output.bin     # on the receiving machine, start first
    python main.py [options] tx input.bin      # on the sending machine
"""

import argparse
import hashlib
import queue
import sys
import time

from packet_framer import ARQPacket, PacketFramer, QueueTransport
from sound_packet_bridge import MinimodemBridge, airtime


def frame_len(payload: int, fec_symbols: int) -> int:
    """Worst-case encoded frame size for a payload: ARQ header + CRC + RS parity + COBS overhead."""
    raw = ARQPacket.HEADER_SIZE + payload + PacketFramer.CRC_SIZE + fec_symbols
    return raw + raw // 254 + 2


def ack_timeout(args) -> float:
    """Time to wait for an ACK: data frame + ACK frame on air, two turnarounds, plus slack."""
    fec = args.fec_symbols if args.fec else 0
    return (airtime(frame_len(args.payload_size, fec), args.baud)
            + airtime(frame_len(0, fec), args.baud) + 2 * args.turnaround + 3.0)


def build(args):
    """Create and start transport + bridge, return (framer, bridge, transport)."""
    tx_q, rx_q = queue.Queue(), queue.Queue()
    framer = PacketFramer(max_payload_length=args.payload_size, use_fec=args.fec,
                          fec_nsym=args.fec_symbols, use_arq=args.arq,
                          arq_timeout=args.timeout, arq_max_retries=args.max_retries)
    transport = QueueTransport(tx_q, rx_q)
    framer.set_transport(transport)
    bridge = MinimodemBridge(args.minimodem, tx_q, rx_q, baud_rate=args.baud, mark=args.mark,
                             space=args.space, volume=args.volume, turnaround=args.turnaround)
    transport.start()
    bridge.start()
    return framer, bridge, transport


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()[:16]


def transmitter_mode(args, framer, bridge) -> bool:
    data = sys.stdin.buffer.read() if args.input_file == '-' else open(args.input_file, 'rb').read()
    print(f"Read {len(data)} bytes from {args.input_file} (sha256 {digest(data)})")
    started = time.time()
    ok = framer.send_data(data)
    if not args.arq:  # nothing to wait for but the queue draining
        while not bridge.tx_queue.empty():
            time.sleep(0.2)
        time.sleep(args.turnaround + 1)
    took = time.time() - started
    print(f"{'Sent' if ok else 'FAILED to send'} {len(data)} bytes in {took:.0f}s "
          f"({8 * len(data) / took:.1f} bit/s effective)")
    return ok


def receiver_mode(args, framer, bridge) -> bool:
    print("Waiting for data...")
    deadline = time.time() + args.rx_timeout
    while not framer.all_packets_received and time.time() < deadline:
        time.sleep(0.1)
    if framer.all_packets_received:
        # Stay a bit so a lost final ACK can be re-sent when the sender retries EOT
        end = time.time() + args.timeout + args.turnaround
        while time.time() < end or not bridge.tx_queue.empty():
            time.sleep(0.1)
    else:
        print(f"Timeout after {args.rx_timeout:.0f}s")
    data = framer.get_received_data()
    if args.output_file == '-':
        sys.stdout.buffer.write(data)
    else:
        with open(args.output_file, 'wb') as f:
            f.write(data)
    print(f"Wrote {len(data)} bytes to {args.output_file} (sha256 {digest(data)})")
    return framer.all_packets_received


def parse_args():
    p = argparse.ArgumentParser(description='Sound Modem - Data Transmission over Audio')
    p.add_argument('--minimodem', default='minimodem', help='Path to minimodem executable')
    p.add_argument('--baud', type=float, default=100, help='Baud rate (default: 100)')
    p.add_argument('--mark', type=float, help='Mark (1) tone in Hz (default: minimodem\'s)')
    p.add_argument('--space', type=float, help='Space (0) tone in Hz (default: minimodem\'s)')
    p.add_argument('--volume', type=float, help='TX amplitude 0..1 (default: minimodem\'s)')
    p.add_argument('--payload-size', type=int, default=64, help='Payload bytes per packet (default: 64)')
    p.add_argument('--fec', action='store_true', help='Enable Reed-Solomon forward error correction')
    p.add_argument('--fec-symbols', type=int, default=16, help='Reed-Solomon parity bytes (default: 16)')
    p.add_argument('--arq', action='store_true', help='Enable Stop-and-Wait ARQ')
    p.add_argument('--timeout', type=float, help='ARQ ACK timeout in s (default: computed from airtime)')
    p.add_argument('--max-retries', type=int, default=5, help='ARQ retransmissions per packet (default: 5)')
    p.add_argument('--turnaround', type=float, default=1.0,
                   help='Pause before transmitting so the peer can switch to RX (default: 1.0)')
    p.add_argument('--rx-timeout', type=float, default=900, help='Receiver gives up after this many s')
    sub = p.add_subparsers(dest='mode', required=True)
    sub.add_parser('tx', help='Transmit a file').add_argument('input_file', help='File to send (- for stdin)')
    sub.add_parser('rx', help='Receive a file').add_argument('output_file', help='Output file (- for stdout)')
    args = p.parse_args()
    if args.timeout is None:
        args.timeout = ack_timeout(args)
    return args


def main():
    args = parse_args()
    print(f"baud={args.baud} mark={args.mark} space={args.space} payload={args.payload_size} "
          f"fec={args.fec_symbols if args.fec else 0} arq={args.arq} ack_timeout={args.timeout:.1f}s")
    framer, bridge, transport = build(args)
    try:
        run = transmitter_mode if args.mode == 'tx' else receiver_mode
        ok = run(args, framer, bridge)
    except KeyboardInterrupt:
        ok = False
    finally:
        bridge.stop()
        transport.stop()
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
