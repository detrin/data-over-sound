#!/usr/bin/env python3
"""
Sound Modem - Main Entry Point

This script integrates packet_framer.py and sound_packet_bridge.py into a single application,
allowing users to transmit data over audio with a single command.
"""

import os
import sys
import time
import argparse
import threading
import queue
import signal
import subprocess
from typing import List, Tuple, Optional, Callable

# Import the components
sys.path.append(os.path.dirname(os.path.abspath(__file__)))
from packet_framer import PacketFramer, QueueTransport
from sound_packet_bridge import MinimodemBridge


def transmitter_mode(args):
    """Run in transmitter mode."""
    print(f"Reading from {args.input_file}...")
    
    # Read input data
    if args.input_file == '-':
        data = sys.stdin.buffer.read()
    else:
        with open(args.input_file, 'rb') as f:
            data = f.read()
    
    print(f"Read {len(data)} bytes")
    
    # Create queues for communication between components
    to_minimodem_queue = queue.Queue()  # Data to be sent to minimodem
    from_minimodem_queue = queue.Queue()  # Data received from minimodem
    
    # Create transport for packet framer
    transport = QueueTransport(to_minimodem_queue, from_minimodem_queue)
    
    # Create packet framer with the correct parameters
    framer = PacketFramer(
        max_payload_length=args.payload_size,
        use_fec=args.fec,
        fec_nsym=args.fec_symbols,
        use_arq=args.arq,
        arq_timeout=args.timeout,
        arq_max_retries=args.max_retries
    )
    
    # Set the transport for the framer
    framer.set_transport(transport)
    
    # Create minimodem bridge (in transmitter mode)
    bridge = MinimodemBridge(
        minimodem_path=args.minimodem,
        tx_queue=to_minimodem_queue,
        rx_queue=from_minimodem_queue,
        baud_rate=args.baud,
        is_server=False,  # Client/transmitter mode
        ack_delay=args.ack_delay,
        debug=args.debug,
        shell=args.shell,
        volume=args.volume
    )
    
    # Start the transport
    transport.start()
    
    # Start the bridge in a separate thread
    bridge_thread = threading.Thread(target=bridge.start)
    bridge_thread.daemon = True
    bridge_thread.start()
    
    # Wait for the bridge to be ready
    print("Waiting for sound bridge to be ready...")
    time.sleep(3)  # Increased wait time to ensure bridge is ready
    
    # Check if the bridge thread is still alive
    if not bridge_thread.is_alive():
        print("Error: Sound bridge failed to start")
        transport.stop()
        return
    
    # Send the data
    print(f"Sending {len(data)} bytes...")
    framer.send_data(data)
    
    # Wait for transmission to complete
    if args.arq:
        print("Waiting for ARQ to complete...")
        max_wait_time = args.timeout * args.max_retries * 2  # seconds
        start_time = time.time()
        
        while not framer.arq.is_transmission_complete():
            time.sleep(0.1)
            
            # Check if the bridge thread is still alive
            if not bridge_thread.is_alive():
                print("Error: Sound bridge stopped unexpectedly")
                break
            
            # Print progress
            current_time = time.time()
            if current_time - start_time > max_wait_time:
                print(f"Timeout after {max_wait_time} seconds")
                break
    else:
        # Without ARQ, just wait a bit for all data to be sent
        print("Waiting for transmission to complete...")
        time.sleep(5)
    
    # Stop the components
    print("Stopping components...")
    bridge.stop()
    transport.stop()
    
    print("Transmission complete")


def receiver_mode(args):
    """Run in receiver mode."""
    print("Waiting for data...")
    
    # Create queues for communication between components
    to_minimodem_queue = queue.Queue()  # Data to be sent to minimodem
    from_minimodem_queue = queue.Queue()  # Data received from minimodem
    
    # Create transport for packet framer
    transport = QueueTransport(to_minimodem_queue, from_minimodem_queue)
    
    # Create packet framer with the correct parameters
    framer = PacketFramer(
        max_payload_length=args.payload_size,
        use_fec=args.fec,
        fec_nsym=args.fec_symbols,
        use_arq=args.arq,
        arq_timeout=args.timeout,
        arq_max_retries=args.max_retries
    )
    
    # Set the transport for the framer
    framer.set_transport(transport)
    
    # Create minimodem bridge
    bridge = MinimodemBridge(
        minimodem_path=args.minimodem,
        tx_queue=to_minimodem_queue,
        rx_queue=from_minimodem_queue,
        baud_rate=args.baud,
        is_server=True,  # Server/receiver mode
        ack_delay=args.ack_delay,
        debug=args.debug,
        shell=args.shell,
        volume=args.volume
    )
    
    # Start the transport
    transport.start()
    
    # Start the bridge in a separate thread
    bridge_thread = threading.Thread(target=bridge.start)
    bridge_thread.daemon = True
    bridge_thread.start()
    
    # Wait for the bridge to be ready
    print("Waiting for sound bridge to be ready...")
    time.sleep(3)  # Increased wait time to ensure bridge is ready
    
    # Check if the bridge thread is still alive
    if not bridge_thread.is_alive():
        print("Error: Sound bridge failed to start")
        transport.stop()
        return
    
    # Wait for data
    print("Waiting for data...")
    max_wait_time = 300.0  # seconds
    start_time = time.time()
    
    while not framer.all_packets_received:
        time.sleep(0.1)
        
        # Check if the bridge thread is still alive
        if not bridge_thread.is_alive():
            print("Error: Sound bridge stopped unexpectedly")
            break
        
        # Print progress
        current_time = time.time()
        if current_time - start_time > max_wait_time:
            print(f"Timeout after {max_wait_time} seconds")
            break
    
    # Get the received data
    received_data = framer.get_received_data()
    
    # Write output data
    if args.output_file == '-':
        sys.stdout.buffer.write(received_data)
    else:
        with open(args.output_file, 'wb') as f:
            f.write(received_data)
    
    print(f"Wrote {len(received_data)} bytes to {args.output_file}")
    
    # Stop the components
    print("Stopping components...")
    bridge.stop()
    transport.stop()


def parse_args():
    """Parse command-line arguments."""
    parser = argparse.ArgumentParser(
        description='Sound Modem - Data Transmission over Audio'
    )
    
    # Common arguments
    parser.add_argument('--minimodem', default='minimodem',
                        help='Path to minimodem executable (default: minimodem)')
    parser.add_argument('--baud', type=int, default=45,
                        help='Baud rate for minimodem (default: 45)')
    parser.add_argument('--payload-size', type=int, default=128,
                        help='Maximum payload size in bytes (default: 128)')
    parser.add_argument('--fec', action='store_true',
                        help='Enable Reed-Solomon forward error correction')
    parser.add_argument('--fec-symbols', type=int, default=32,
                        help='Number of Reed-Solomon error correction symbols (default: 32)')
    parser.add_argument('--arq', action='store_true',
                        help='Enable Stop-and-Wait ARQ')
    parser.add_argument('--timeout', type=float, default=10.0,
                        help='ARQ timeout in seconds (default: 10.0)')
    parser.add_argument('--max-retries', type=int, default=5,
                        help='Maximum number of ARQ retransmission attempts (default: 5)')
    parser.add_argument('--ack-delay', type=float, default=5.0,
                        help='Delay after transmission to wait for ACKs/NAKs (default: 5.0)')
    parser.add_argument('--debug', action='store_true',
                        help='Enable debug output')
    parser.add_argument('--shell', action='store_true',
                        help='Run minimodem through a shell')
    parser.add_argument('--volume', type=float, default=1.0,
                        help='Audio volume for transmission (default: 1.0)')
    
    # Subparsers for transmitter and receiver modes
    subparsers = parser.add_subparsers(dest='mode', required=True,
                                       help='Operating mode')
    
    # Transmitter mode
    tx_parser = subparsers.add_parser('tx', help='Transmitter mode')
    tx_parser.add_argument('input_file', 
                          help='Input file to transmit (use - for stdin)')
    
    # Receiver mode
    rx_parser = subparsers.add_parser('rx', help='Receiver mode')
    rx_parser.add_argument('output_file',
                          help='Output file to write received data (use - for stdout)')
    
    args = parser.parse_args()
    return args


def main():
    """Main entry point."""
    args = parse_args()
    
    # Create signal handler for graceful shutdown
    def signal_handler(sig, frame):
        print("\nInterrupted by user, shutting down...")
        sys.exit(0)
    
    # Register signal handlers
    signal.signal(signal.SIGINT, signal_handler)
    signal.signal(signal.SIGTERM, signal_handler)
    
    # Run in the appropriate mode
    if args.mode == 'tx':
        transmitter_mode(args)
    elif args.mode == 'rx':
        receiver_mode(args)


if __name__ == "__main__":
    main() 