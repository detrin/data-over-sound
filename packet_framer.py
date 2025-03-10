#!/usr/bin/env python3
"""
Packet Framing Library

This module provides implementations of:
- COBS (Consistent Overhead Byte Stuffing) for framing
- Reed-Solomon for forward error correction
- Selective Repeat ARQ for reliable transmission
- Direct queue-based communication with sound_packet_bridge.py
"""

import struct
import binascii
import random
import time
import threading
import queue
import argparse
import os
import sys
from typing import List, Tuple, Optional, Union, ByteString, Dict, Callable, Set
from enum import Enum

try:
    import reedsolo
    REEDSOLO_AVAILABLE = True
except ImportError:
    REEDSOLO_AVAILABLE = False
    print("Warning: Reed-Solomon library not available. Install with 'pip install reedsolo'")


class FramingError(Exception):
    """Exception raised for errors in the framing process"""
    pass


class ARQError(Exception):
    """Exception raised for errors in the ARQ protocol"""
    pass


class PacketType(Enum):
    """Types of packets in the ARQ protocol"""
    DATA = 0
    ACK = 1
    NAK = 2
    EOT = 3  # End of Transmission


class COBSFramer:
    """
    Consistent Overhead Byte Stuffing (COBS) framer.
    
    This class provides methods for framing and unframing data using COBS.
    COBS ensures that the framed data does not contain any zero bytes except
    for the delimiter at the end.
    """
    
    DELIMITER = b'\x00'
    
    @staticmethod
    def frame(data: bytes) -> bytes:
        """
        Frame data using COBS.
        
        Args:
            data: Data to frame
            
        Returns:
            Framed data with delimiter
        """
        if not data:
            return COBSFramer.DELIMITER
        
        result = bytearray()
        code_index = 0
        result.append(0)  # Placeholder for code
        
        for byte in data:
            if byte == 0:
                # Found a zero byte, update the code and start a new block
                result[code_index] = len(result) - code_index
                code_index = len(result)
                result.append(0)  # Placeholder for next code
            else:
                # Non-zero byte, just append it
                result.append(byte)
                
                # If we've reached the maximum block size, start a new block
                if len(result) - code_index == 254:
                    result[code_index] = 255
                    code_index = len(result)
                    result.append(0)  # Placeholder for next code
        
        # Update the last code
        result[code_index] = len(result) - code_index
        
        # Add the delimiter
        result.append(0)
        
        return bytes(result)
    
    @staticmethod
    def unframe(framed_data: bytes) -> Optional[bytes]:
        """
        Unframe data using COBS.
        
        Args:
            framed_data: Framed data with delimiter
            
        Returns:
            Unframed data, or None if the framed data is invalid
        """
        # Add more robust error handling
        try:
            # Check if the data is too short or doesn't end with the delimiter
            if not framed_data or len(framed_data) < 2:
                print(f"Received data too short: {len(framed_data) if framed_data else 0} bytes")
                return None
            
            # Find the delimiter
            delimiter_pos = framed_data.find(COBSFramer.DELIMITER)
            if delimiter_pos == -1:
                print(f"No delimiter found in received data ({len(framed_data)} bytes)")
                return None
            
            # Use only the data up to the delimiter
            framed_data = framed_data[:delimiter_pos+1]
            
            # If it's just a delimiter, return empty bytes
            if framed_data == COBSFramer.DELIMITER:
                return b''
            
            result = bytearray()
            i = 0
            
            while i < len(framed_data) - 1:  # Skip the trailing delimiter
                if framed_data[i] == 0:
                    print(f"Invalid COBS encoding: zero code at position {i}")
                    return None
                
                code = framed_data[i]
                
                # Check if the code points beyond the end of the data
                if i + code > len(framed_data):
                    print(f"Invalid COBS encoding: code {code} at position {i} points beyond end of data")
                    return None
                
                # Copy the data
                for j in range(1, code):
                    if i + j < len(framed_data) - 1:
                        result.append(framed_data[i + j])
                
                # If the code is less than 255, add a zero byte
                if code < 255 and i + code < len(framed_data):
                    result.append(0)
                
                i += code
            
            return bytes(result)
        except Exception as e:
            print(f"Error in COBS unframing: {e}")
            return None


class ARQPacket:
    """
    ARQ packet for reliable transmission.
    
    This class represents a packet in the ARQ protocol, with a type, sequence number,
    total packet count, and payload.
    """
    
    HEADER_SIZE = 9  # 1 byte for type, 4 bytes for seq_num, 4 bytes for total_packets
    
    def __init__(self, packet_type: PacketType, seq_num: int, total_packets: int, payload: bytes = b''):
        """
        Initialize an ARQ packet.
        
        Args:
            packet_type: Type of packet (DATA, ACK, NAK, EOT)
            seq_num: Sequence number
            total_packets: Total number of packets
            payload: Packet payload (for DATA packets)
        """
        self.packet_type = packet_type
        self.seq_num = seq_num
        self.total_packets = total_packets
        self.payload = payload
    
    def to_bytes(self) -> bytes:
        """
        Convert the packet to bytes.
        
        Returns:
            Packet as bytes
        """
        header = struct.pack("!BII", self.packet_type.value, self.seq_num, self.total_packets)
        return header + self.payload
    
    @classmethod
    def from_bytes(cls, data: bytes) -> 'ARQPacket':
        """
        Create a packet from bytes.
        
        Args:
            data: Packet as bytes
            
        Returns:
            ARQ packet
            
        Raises:
            ARQError: If the packet is invalid
        """
        if len(data) < cls.HEADER_SIZE:
            raise ARQError(f"Packet too short: {len(data)} bytes")
        
        try:
            packet_type_value, seq_num, total_packets = struct.unpack("!BII", data[:cls.HEADER_SIZE])
            packet_type = PacketType(packet_type_value)
            payload = data[cls.HEADER_SIZE:]
            return cls(packet_type, seq_num, total_packets, payload)
        except (struct.error, ValueError) as e:
            raise ARQError(f"Invalid packet format: {e}")


class StopAndWaitARQ:
    """
    Stop-and-Wait ARQ protocol implementation.
    
    This class implements a simple Stop-and-Wait ARQ protocol for reliable transmission
    over unreliable channels. It sends one packet at a time and waits for an acknowledgment
    before sending the next packet.
    """
    
    def __init__(self, 
                 transport=None,
                 timeout: float = 10.0,
                 max_retries: int = 5,
                 send_callback: Callable[[bytes], None] = None,
                 receive_callback: Callable[[bytes], None] = None):
        """
        Initialize the Stop-and-Wait ARQ protocol.
        
        Args:
            transport: Transport to use for sending/receiving packets
            timeout: Timeout for retransmissions in seconds
            max_retries: Maximum number of retransmission attempts
            send_callback: Callback function for sending packets
            receive_callback: Callback function for received data
        """
        self.transport = transport
        self.timeout = timeout
        self.max_retries = max_retries
        self.send_callback = send_callback
        self.receive_callback = receive_callback
        
        # Sequence numbers
        self.send_seq = 0
        self.recv_seq = 0
        
        # Transmission state
        self.current_packet = None
        self.last_send_time = 0
        self.retries = 0
        self.transmission_complete = False
        
        # Received data
        self.received_data = bytearray()
        
        # Locks for thread safety
        self.send_lock = threading.Lock()
        self.recv_lock = threading.Lock()
        
        # Timer thread for retransmissions
        self.timer_thread = None
        self.running = False
    
    def start(self):
        """Start the ARQ protocol."""
        self.running = True
        self.timer_thread = threading.Thread(target=self._timer_thread)
        self.timer_thread.daemon = True
        self.timer_thread.start()
    
    def stop(self):
        """Stop the ARQ protocol."""
        self.running = False
        if self.timer_thread:
            self.timer_thread.join(timeout=1.0)
    
    def send_data(self, data: bytes):
        """
        Send data using the ARQ protocol.
        
        Args:
            data: Data to send
        """
        # Split data into chunks
        chunk_size = 128  # Maximum payload size
        data_chunks = [data[i:i+chunk_size] for i in range(0, len(data), chunk_size)]
        
        with self.send_lock:
            print(f"Sending {len(data_chunks)} packets...")
            
            # Send each chunk one at a time, waiting for ACK
            for i, chunk in enumerate(data_chunks):
                # Create packet
                packet = ARQPacket(
                    packet_type=PacketType.DATA,
                    seq_num=self.send_seq,
                    total_packets=len(data_chunks),
                    payload=chunk
                )
                
                # Send packet and wait for ACK
                self._send_and_wait_for_ack(packet)
                
                # Update sequence number
                self.send_seq = (self.send_seq + 1) % 256
                
                # Print progress
                print(f"Sent packet {i+1}/{len(data_chunks)}")
            
            # Send EOT packet
            eot_packet = ARQPacket(
                packet_type=PacketType.EOT,
                seq_num=self.send_seq,
                total_packets=len(data_chunks)
            )
            self._send_packet(eot_packet)
            
            # Mark transmission as complete
            self.transmission_complete = True
            print("Transmission complete")
    
    def _send_and_wait_for_ack(self, packet: ARQPacket):
        """
        Send a packet and wait for acknowledgment.
        
        Args:
            packet: Packet to send
        """
        with self.send_lock:
            # Store the current packet for retransmissions
            self.current_packet = packet
            self.retries = 0
            
            # Send the packet
            self._send_packet(packet)
            
            # Wait for ACK (handled by timer thread)
            while self.current_packet and self.running:
                time.sleep(0.1)
    
    def _send_packet(self, packet: ARQPacket):
        """
        Send a packet.
        
        Args:
            packet: Packet to send
        """
        # Convert packet to bytes and frame it
        packet_bytes = COBSFramer.frame(packet.to_bytes())
        
        print(f"Sending {packet.packet_type.name} packet {packet.seq_num}/{packet.total_packets}")
        
        # Update last send time
        self.last_send_time = time.time()
        
        # Use the callback if available, otherwise use the transport directly
        if self.send_callback:
            self.send_callback(packet_bytes)
        elif self.transport and hasattr(self.transport, 'send'):
            self.transport.send(packet_bytes)
        else:
            print("Warning: No send callback or transport available")
        
        # Add a small delay to ensure the packet is sent
        time.sleep(0.1)
    
    def process_packet(self, packet: ARQPacket):
        """
        Process a received packet.
        
        Args:
            packet: Received packet
        """
        with self.recv_lock:
            if packet.packet_type == PacketType.DATA:
                # Process DATA packet
                print(f"Received DATA packet {packet.seq_num}/{packet.total_packets}")
                
                # Check if this is the expected sequence number
                if packet.seq_num == self.recv_seq:
                    # Store the data
                    self.received_data.extend(packet.payload)
                    
                    # Update sequence number
                    self.recv_seq = (self.recv_seq + 1) % 256
                    
                    # Deliver the data to the application
                    if self.receive_callback:
                        self.receive_callback(packet.payload)
                
                # Send ACK
                ack_packet = ARQPacket(
                    packet_type=PacketType.ACK,
                    seq_num=packet.seq_num,
                    total_packets=packet.total_packets
                )
                self._send_packet(ack_packet)
                
            elif packet.packet_type == PacketType.ACK:
                # Process ACK packet
                print(f"Received ACK for packet {packet.seq_num}")
                
                # Check if this ACK is for the current packet
                if self.current_packet and packet.seq_num == self.current_packet.seq_num:
                    # Clear current packet to indicate it was acknowledged
                    self.current_packet = None
                
            elif packet.packet_type == PacketType.NAK:
                # Process NAK packet
                print(f"Received NAK for packet {packet.seq_num}")
                
                # Check if this NAK is for the current packet
                if self.current_packet and packet.seq_num == self.current_packet.seq_num:
                    # Retransmit the packet
                    self._send_packet(self.current_packet)
                    self.retries += 1
                
            elif packet.packet_type == PacketType.EOT:
                # Process EOT packet
                print(f"Received EOT packet, total packets: {packet.total_packets}")
                
                # Mark transmission as complete
                self.transmission_complete = True
    
    def _timer_thread(self):
        """Timer thread for handling retransmissions."""
        while self.running:
            try:
                # Check if we have a packet waiting for ACK
                if self.current_packet:
                    # Check if it has timed out
                    current_time = time.time()
                    if current_time - self.last_send_time > self.timeout:
                        # Check if we've exceeded the maximum retries
                        if self.retries >= self.max_retries:
                            print(f"Maximum retries ({self.max_retries}) exceeded for packet {self.current_packet.seq_num}")
                            # Clear current packet to move on
                            self.current_packet = None
                        else:
                            # Retransmit the packet
                            print(f"Timeout, retransmitting packet {self.current_packet.seq_num} (retry {self.retries+1}/{self.max_retries})")
                            self._send_packet(self.current_packet)
                            self.retries += 1
            except Exception as e:
                print(f"Error in timer thread: {e}")
            
            # Sleep for a short time
            time.sleep(0.1)
    
    def is_transmission_complete(self):
        """Check if transmission is complete."""
        return self.transmission_complete
    
    def get_received_data(self):
        """Get the received data."""
        with self.recv_lock:
            return bytes(self.received_data)


class QueueTransport:
    """
    Queue-based transport for packet framer.
    
    This class provides a transport implementation that uses queues for
    sending and receiving data.
    """
    
    def __init__(self, tx_queue: queue.Queue, rx_queue: queue.Queue):
        """
        Initialize the queue transport.
        
        Args:
            tx_queue: Queue for data to be transmitted
            rx_queue: Queue for received data
        """
        self.tx_queue = tx_queue
        self.rx_queue = rx_queue
        self.running = False
        self.rx_thread = None
    
    def start(self):
        """Start the transport."""
        self.running = True
        self.rx_thread = threading.Thread(target=self._rx_thread)
        self.rx_thread.daemon = True
        self.rx_thread.start()
        print("Queue transport started")
    
    def stop(self):
        """Stop the transport."""
        self.running = False
        if self.rx_thread:
            self.rx_thread.join(timeout=1.0)
        print("Queue transport stopped")
    
    def send(self, data: bytes):
        """
        Send data.
        
        Args:
            data: Data to send
        """
        # Add the data to the TX queue
        self.tx_queue.put(data)
        print(f"Added {len(data)} bytes to TX queue (size: {self.tx_queue.qsize()})")
    
    def _rx_thread(self):
        """Thread to handle received data."""
        while self.running:
            try:
                # Try to get data from the RX queue with a timeout
                try:
                    data = self.rx_queue.get(timeout=0.1)
                except queue.Empty:
                    continue
                
                # Process the received data
                if hasattr(self, 'receive_callback') and self.receive_callback:
                    self.receive_callback(data)
                
                # Mark the task as done
                self.rx_queue.task_done()
            
            except Exception as e:
                print(f"Error in RX thread: {e}")
                time.sleep(0.1)


class PacketFramer:
    """
    Packet framer with Reed-Solomon FEC and ARQ.
    
    This class provides packet framing with optional Reed-Solomon forward error
    correction and ARQ for reliable transmission.
    """
    
    def __init__(self, 
                 max_payload_length: int = 128,
                 use_fec: bool = False,
                 fec_nsym: int = 32,
                 use_arq: bool = False,
                 arq_timeout: float = 10.0,
                 arq_max_retries: int = 5):
        """
        Initialize the packet framer.
        
        Args:
            max_payload_length: Maximum payload length in bytes
            use_fec: Whether to use Reed-Solomon FEC
            fec_nsym: Number of Reed-Solomon error correction symbols
            use_arq: Whether to use ARQ
            arq_timeout: ARQ timeout in seconds
            arq_max_retries: Maximum number of ARQ retransmission attempts
        """
        self.max_payload_length = max_payload_length
        self.use_fec = use_fec
        self.fec_nsym = fec_nsym
        self.use_arq = use_arq
        self.arq_timeout = arq_timeout
        self.arq_max_retries = arq_max_retries
        
        # Initialize Reed-Solomon codec if FEC is enabled
        if self.use_fec:
            if not REEDSOLO_AVAILABLE:
                raise ImportError("Reed-Solomon library not available")
            self.rs = reedsolo.RSCodec(self.fec_nsym)
        
        # Initialize ARQ if enabled
        self.arq = None
        self.transport = None
        
        # Received data
        self.received_data = bytearray()
        self.all_packets_received = False
    
    def set_transport(self, transport):
        """
        Set the transport for the packet framer.
        
        Args:
            transport: Transport to use for sending/receiving packets
        """
        self.transport = transport
        
        # Initialize ARQ if enabled
        if self.use_arq:
            self.arq = StopAndWaitARQ(
                transport=transport,
                timeout=self.arq_timeout,
                max_retries=self.arq_max_retries
            )
            self.arq.start()
    
    def start(self, send_callback: Callable[[bytes], None], receive_callback: Callable[[bytes], None]):
        """
        Start the packet framer.
        
        Args:
            send_callback: Callback function for sending packets
            receive_callback: Callback function for received data
        """
        # Set up ARQ if enabled
        if self.use_arq and self.arq:
            self.arq.send_callback = send_callback
            self.arq.receive_callback = receive_callback
        else:
            # If ARQ is not enabled, use the callbacks directly
            self.send_callback = send_callback
            self.receive_callback = receive_callback
    
    def send_data(self, data: bytes):
        """
        Send data.
        
        Args:
            data: Data to send
        """
        print(f"Sending {len(data)} bytes...")
        
        # If using ARQ, let it handle the transmission
        if self.use_arq and self.arq:
            self.arq.send_data(data)
            return
        
        # Split data into chunks
        chunks = []
        for i in range(0, len(data), self.max_payload_length):
            chunk = data[i:i+self.max_payload_length]
            chunks.append(chunk)
        
        # Send each chunk
        for i, chunk in enumerate(chunks):
            # Apply FEC if enabled
            if self.use_fec:
                chunk = self.rs.encode(chunk)
            
            # Frame the chunk
            framed_chunk = COBSFramer.frame(chunk)
            
            # Send the framed chunk
            if self.transport and hasattr(self.transport, 'send'):
                self.transport.send(framed_chunk)
            elif hasattr(self, 'send_callback') and self.send_callback:
                self.send_callback(framed_chunk)
            
            # Print progress
            print(f"Sent chunk {i+1}/{len(chunks)}")
    
    def process_incoming_data(self, data: bytes):
        """
        Process incoming data.
        
        Args:
            data: Incoming data
        """
        try:
            # Try to unframe the data
            unframed_data = COBSFramer.unframe(data)
            if unframed_data is None:
                print("Error unframing data")
                return
            
            # If using ARQ, process the packet through the ARQ protocol
            if self.use_arq and self.arq:
                # Try to parse the packet
                try:
                    packet = ARQPacket.from_bytes(unframed_data)
                    print(f"Received {packet.packet_type.name} packet {packet.seq_num}/{packet.total_packets}")
                    
                    # Process the packet through the ARQ protocol
                    self.arq.process_packet(packet)
                    
                    # If this is an EOT packet, mark all packets as received
                    if packet.packet_type == PacketType.EOT:
                        self.all_packets_received = True
                    
                    return
                except Exception as e:
                    print(f"Error parsing ARQ packet: {e}")
                    return
            
            # If not using ARQ, process the data directly
            # Apply FEC if enabled
            if self.use_fec:
                try:
                    unframed_data = self.rs.decode(unframed_data)[0]
                except reedsolo.ReedSolomonError as e:
                    print(f"Reed-Solomon decoding error: {e}")
                    return
            
            # Store the data
            self.received_data.extend(unframed_data)
            
            # Deliver the data to the application
            if hasattr(self, 'receive_callback') and self.receive_callback:
                self.receive_callback(unframed_data)
            
        except Exception as e:
            print(f"Error processing incoming data: {e}")
    
    def get_received_data(self):
        """
        Get the received data.
        
        Returns:
            Received data as bytes
        """
        if self.use_arq and self.arq:
            return self.arq.get_received_data()
        else:
            return bytes(self.received_data)


def transmitter_mode(args, tx_queue, rx_queue):
    """Run in transmitter mode."""
    print(f"Reading from {args.input_file}...")
    
    # Read input data
    if args.input_file == '-':
        data = sys.stdin.buffer.read()
    else:
        with open(args.input_file, 'rb') as f:
            data = f.read()
    
    print(f"Read {len(data)} bytes")
    
    # Create transport
    transport = QueueTransport(tx_queue, rx_queue)
    
    # Create packet framer
    framer = PacketFramer(
        max_payload_length=args.payload_size,
        use_fec=args.fec,
        fec_nsym=args.fec_symbols,
        use_arq=args.arq,
        arq_timeout=args.timeout
    )
    
    # Start the transport
    transport.start()
    
    # Wait for the sound bridge to be ready
    print("Waiting for sound bridge to be ready...")
    time.sleep(2)
    
    # Send the data
    print(f"Sending {len(data)} bytes...")
    framer.send_data(data)
    
    # Wait for transmission to complete
    if args.arq:
        print("Waiting for ARQ to complete...")
        max_wait_time = 60.0  # seconds
        start_time = time.time()
        
        while not framer.arq.is_transmission_complete():
            time.sleep(0.1)
            
            # Print progress
            current_time = time.time()
            if current_time - start_time > max_wait_time:
                print(f"Timeout after {max_wait_time} seconds")
                break
    
    # Stop the transport
    transport.stop()
    
    print("Transmission complete")


def receiver_mode(args, tx_queue, rx_queue):
    """Run in receiver mode."""
    print("Waiting for data...")
    
    # Create transport
    transport = QueueTransport(tx_queue, rx_queue)
    
    # Create packet framer
    framer = PacketFramer(
        max_payload_length=args.payload_size,
        use_fec=args.fec,
        fec_nsym=args.fec_symbols,
        use_arq=args.arq,
        arq_timeout=args.timeout
    )
    
    # Start the transport
    transport.start()
    
    # Wait for data
    print("Waiting for data...")
    max_wait_time = 300.0  # seconds
    start_time = time.time()
    
    while not framer.all_packets_received:
        time.sleep(0.1)
        
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
    
    # Stop the transport
    transport.stop()


def parse_args():
    """Parse command-line arguments."""
    parser = argparse.ArgumentParser(
        description='Packet Framing with Reed-Solomon FEC and Selective Repeat ARQ'
    )
    
    # Common arguments
    parser.add_argument('--payload-size', type=int, default=128,
                        help='Maximum payload size in bytes (default: 128)')
    parser.add_argument('--fec', action='store_true',
                        help='Enable Reed-Solomon forward error correction')
    parser.add_argument('--fec-symbols', type=int, default=32,
                        help='Number of Reed-Solomon error correction symbols (default: 32)')
    parser.add_argument('--arq', action='store_true',
                        help='Enable Selective Repeat ARQ')
    parser.add_argument('--timeout', type=float, default=10.0,
                        help='ARQ timeout in seconds (default: 10.0)')
    
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
    
    # Create queues for communication with sound bridge
    tx_queue = queue.Queue()  # Data to be sent to sound bridge
    rx_queue = queue.Queue()  # Data received from sound bridge
    
    # Run in the appropriate mode
    if args.mode == 'tx':
        transmitter_mode(args, tx_queue, rx_queue)
    elif args.mode == 'rx':
        receiver_mode(args, tx_queue, rx_queue)


if __name__ == "__main__":
    main()