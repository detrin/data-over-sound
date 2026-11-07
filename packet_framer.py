#!/usr/bin/env python3
"""
Packet Framing Library

This module provides implementations of:
- COBS (Consistent Overhead Byte Stuffing) for framing
- Reed-Solomon for forward error correction
- CRC32 integrity check
- Stop-and-Wait ARQ for reliable transmission
- Direct queue-based communication with sound_packet_bridge.py
"""

import struct
import zlib
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
                if len(result) - code_index == 255:  # code byte + 254 data bytes
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
            if framed_data == COBSFramer.DELIMITER:
                return b''
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
                if code < 255 and i + code < len(framed_data) - 1:  # no implied zero before the delimiter
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
    Stop-and-Wait ARQ: send one packet, wait for its ACK, retransmit on timeout.

    The EOT packet is acknowledged like DATA, so both sides know the transfer finished.
    `send` is called with raw (unencoded) packet bytes; encoding is the framer's job.
    """

    def __init__(self, send: Callable[[bytes], None], timeout: float = 10.0,
                 max_retries: int = 5, chunk_size: int = 128):
        self.send = send
        self.timeout = timeout
        self.max_retries = max_retries
        self.chunk_size = chunk_size
        self.recv_seq = 0
        self.received_data = bytearray()
        self.finished = threading.Event()
        self._acked = threading.Event()
        self._waiting_for = None
        self._lock = threading.Lock()

    def send_data(self, data: bytes) -> bool:
        """Send data reliably. Blocks until done; returns True if every packet was ACKed."""
        chunks = [data[i:i + self.chunk_size] for i in range(0, len(data), self.chunk_size)]
        total = len(chunks)
        packets = [ARQPacket(PacketType.DATA, seq, total, chunk) for seq, chunk in enumerate(chunks)]
        packets.append(ARQPacket(PacketType.EOT, total, total))
        print(f"Sending {total} packets (+EOT)...")
        ok = all(self._send_reliably(p) for p in packets)
        self.finished.set()
        print("Transmission complete" if ok else "Transmission FAILED")
        return ok

    def _send_reliably(self, packet: ARQPacket) -> bool:
        self._waiting_for = packet.seq_num
        for attempt in range(self.max_retries + 1):
            self._acked.clear()
            note = "" if attempt == 0 else f" (retry {attempt}/{self.max_retries})"
            print(f"Sending {packet.packet_type.name} {packet.seq_num}/{packet.total_packets}{note}")
            self.send(packet.to_bytes())
            if self._acked.wait(self.timeout):
                print(f"ACK {packet.seq_num}/{packet.total_packets}")
                return True
        print(f"Maximum retries ({self.max_retries}) exceeded for packet {packet.seq_num}")
        return False

    def process_packet(self, packet: ARQPacket):
        with self._lock:
            if packet.packet_type == PacketType.ACK:
                if packet.seq_num == self._waiting_for:
                    self._acked.set()
                return
            if packet.packet_type == PacketType.DATA and packet.seq_num == self.recv_seq:
                self.received_data.extend(packet.payload)
                self.recv_seq += 1
                print(f"Received DATA {packet.seq_num + 1}/{packet.total_packets}")
            elif packet.packet_type == PacketType.EOT and packet.seq_num == self.recv_seq:
                print(f"Received EOT, total packets: {packet.total_packets}")
                self.finished.set()
            elif packet.packet_type in (PacketType.DATA, PacketType.EOT):
                print(f"Duplicate {packet.packet_type.name} {packet.seq_num}, re-ACKing")
            else:
                return
            # ACK anything at or below what we've accepted (duplicates mean our ACK was lost)
            if packet.seq_num <= self.recv_seq:
                self.send(ARQPacket(PacketType.ACK, packet.seq_num, packet.total_packets).to_bytes())

    def is_transmission_complete(self) -> bool:
        return self.finished.is_set()

    def get_received_data(self) -> bytes:
        with self._lock:
            return bytes(self.received_data)


class QueueTransport:
    """Queue-based transport: puts outgoing frames on tx_queue, feeds rx_queue frames to receive_callback."""

    def __init__(self, tx_queue: queue.Queue, rx_queue: queue.Queue):
        self.tx_queue = tx_queue
        self.rx_queue = rx_queue
        self.receive_callback: Optional[Callable[[bytes], None]] = None
        self.running = False
        self.rx_thread = None

    def start(self):
        self.running = True
        self.rx_thread = threading.Thread(target=self._rx_loop, daemon=True)
        self.rx_thread.start()

    def stop(self):
        self.running = False
        if self.rx_thread:
            self.rx_thread.join(timeout=1.0)

    def send(self, data: bytes):
        self.tx_queue.put(data)

    def _rx_loop(self):
        while self.running:
            try:
                data = self.rx_queue.get(timeout=0.1)
            except queue.Empty:
                continue
            if self.receive_callback:
                try:
                    self.receive_callback(data)
                except Exception as e:
                    print(f"Error handling received frame: {e}")


class PacketFramer:
    """
    Packet encoding (CRC32 -> optional Reed-Solomon -> COBS) plus optional Stop-and-Wait ARQ.

    Every frame on the wire, DATA or ACK, goes through the same encode/decode path.
    """

    CRC_SIZE = 4

    def __init__(self,
                 max_payload_length: int = 128,
                 use_fec: bool = False,
                 fec_nsym: int = 32,
                 use_arq: bool = False,
                 arq_timeout: float = 10.0,
                 arq_max_retries: int = 5):
        self.max_payload_length = max_payload_length
        self.use_arq = use_arq
        self.rs = None
        if use_fec:
            if not REEDSOLO_AVAILABLE:
                raise ImportError("Reed-Solomon library not available")
            self.rs = reedsolo.RSCodec(fec_nsym)
        self.arq = StopAndWaitARQ(self._send_raw, arq_timeout, arq_max_retries,
                                  max_payload_length) if use_arq else None
        self.transport = None
        self.received_data = bytearray()

    # --- wire encoding -------------------------------------------------
    def encode(self, raw: bytes) -> bytes:
        data = raw + struct.pack("!I", zlib.crc32(raw))
        if self.rs:
            data = bytes(self.rs.encode(data))
        return COBSFramer.frame(data)

    def decode(self, wire: bytes) -> Optional[bytes]:
        data = COBSFramer.unframe(wire)
        if data is None:
            return None
        if self.rs:
            try:
                data = bytes(self.rs.decode(data)[0])
            except reedsolo.ReedSolomonError as e:
                print(f"Reed-Solomon could not correct frame: {e}")
                return None
        raw, crc = data[:-self.CRC_SIZE], data[-self.CRC_SIZE:]
        if len(crc) < self.CRC_SIZE or struct.unpack("!I", crc)[0] != zlib.crc32(raw):
            print("CRC mismatch, dropping frame")
            return None
        return raw

    # --- plumbing ------------------------------------------------------
    def set_transport(self, transport):
        self.transport = transport
        transport.receive_callback = self.process_incoming_data

    def _send_raw(self, raw: bytes):
        self.transport.send(self.encode(raw))

    def send_data(self, data: bytes) -> bool:
        if self.arq:
            return self.arq.send_data(data)
        for i in range(0, len(data), self.max_payload_length):
            self._send_raw(data[i:i + self.max_payload_length])
        return True

    def process_incoming_data(self, wire: bytes):
        raw = self.decode(wire)
        if raw is None:
            return
        if self.arq:
            try:
                self.arq.process_packet(ARQPacket.from_bytes(raw))
            except ARQError as e:
                print(f"Error parsing ARQ packet: {e}")
        else:
            self.received_data.extend(raw)

    @property
    def all_packets_received(self) -> bool:
        return bool(self.arq and self.arq.finished.is_set())

    def get_received_data(self) -> bytes:
        return self.arq.get_received_data() if self.arq else bytes(self.received_data)
