#!/usr/bin/env python3
"""
Sound Packet Bridge

This script bridges Python queues with minimodem for sound-based transmission.
It uses separate processes for sending and receiving data through minimodem,
enabling data transmission over audio (through the air).
"""

import os
import sys
import time
import argparse
import threading
import subprocess
import signal
import queue
import select
from typing import List, Tuple, Optional, Callable
import base64

# Import the packet framer
sys.path.append(os.path.dirname(os.path.abspath(__file__)))
from packet_framer import COBSFramer


class MinimodemBridge:
    """
    Bridge between Python queues and minimodem for sound-based transmission.
    
    This class connects packet_framer.py with minimodem to enable data transmission
    over audio channels (through the air) in half-duplex mode.
    """
    
    # Operating modes
    MODE_TX = 0  # Transmitting mode
    MODE_RX = 1  # Receiving mode
    
    def __init__(self, 
                 minimodem_path: str,
                 tx_queue: queue.Queue,
                 rx_queue: queue.Queue,
                 baud_rate: int = 45,
                 is_server: bool = False,
                 ack_delay: float = 2.0,
                 debug: bool = False,
                 shell: bool = False,
                 volume: float = 1.0):
        """
        Initialize the minimodem bridge.
        
        Args:
            minimodem_path: Path to the minimodem executable
            tx_queue: Queue for data to be transmitted
            rx_queue: Queue for received data
            baud_rate: Baud rate for minimodem (default: 45)
            is_server: Whether this is a server (receiver) or client (transmitter)
            ack_delay: Delay after transmission to wait for ACKs/NAKs (default: 2.0)
            debug: Enable debug output (default: False)
            shell: Run minimodem through a shell (default: False)
            volume: Audio volume for transmission (default: 1.0)
        """
        self.minimodem_path = minimodem_path
        self.tx_queue = tx_queue
        self.rx_queue = rx_queue
        self.baud_rate = baud_rate
        self.is_server = is_server
        self.ack_delay = ack_delay
        self.debug = debug
        self.shell = shell
        self.volume = volume
        
        # Check if minimodem exists and is executable
        if not self.shell and not os.path.exists(minimodem_path):
            raise FileNotFoundError(f"Minimodem executable not found at {minimodem_path}")
        
        if not self.shell and not os.access(minimodem_path, os.X_OK):
            raise PermissionError(f"Minimodem executable at {minimodem_path} is not executable")
        
        # Current minimodem process
        self.process = None
        
        # Current operating mode
        self.current_mode = self.MODE_RX if is_server else self.MODE_TX
        
        # Control flags
        self.running = False
        self.is_transmitting = False
        
        # Timing
        self.last_tx_time = 0
        self.last_rx_time = 0
        self.last_mode_switch_time = 0
        
        # Buffer for received data
        self.rx_buffer = bytearray()
        
        # Mode switching control
        self.mode_switch_lock = threading.Lock()
        
        # Processing thread
        self.processing_thread = None
    
    def start(self):
        """
        Start the minimodem bridge.
        """
        self.running = True
        
        # Initialize timing variables
        self.last_rx_time = time.time()
        self.last_tx_time = time.time()
        
        # Start in the appropriate mode
        if self.is_server:
            success = self._switch_to_rx_mode()
            if not success:
                print("Failed to start RX process, check if minimodem is installed correctly")
                self.running = False
                return
        else:
            success = self._switch_to_tx_mode()
            if not success:
                print("Failed to start TX process, check if minimodem is installed correctly")
                self.running = False
                return
        
        # Start the processing thread
        self.processing_thread = threading.Thread(target=self._processing_thread)
        self.processing_thread.daemon = True
        self.processing_thread.start()
        
        # Keep running until stopped
        try:
            print("Bridge running. Press Ctrl+C to stop.")
            while self.running:
                time.sleep(0.1)
        except KeyboardInterrupt:
            print("\nBridge interrupted by user")
        finally:
            self.stop()
    
    def stop(self):
        """
        Stop the minimodem bridge.
        """
        print("Stopping minimodem bridge...")
        self.running = False
        
        # Terminate the current process
        if self.process:
            try:
                self.process.terminate()
                self.process.wait(timeout=1)
            except:
                pass
        
        # Wait for the processing thread to exit
        if self.processing_thread and self.processing_thread.is_alive():
            self.processing_thread.join(timeout=1)
        
        print("Minimodem bridge stopped")
    
    def _switch_to_tx_mode(self):
        """Switch to transmitter mode. Returns True if successful, False otherwise."""
        with self.mode_switch_lock:
            if self.current_mode == self.MODE_TX and self.process:
                return True  # Already in TX mode
            
            print("Switching to TX mode")
            
            # Terminate the current process if it exists
            if self.process:
                try:
                    self.process.terminate()
                    self.process.wait(timeout=1)
                except:
                    pass
                self.process = None  # Ensure process is cleared
            
            # Start the TX process
            if self.shell:
                tx_cmd = f"{self.minimodem_path} -t --quiet {self.baud_rate}"
            else:
                tx_cmd = [
                    self.minimodem_path,
                    '-t',  # Transmit mode
                    '--quiet',
                    str(self.baud_rate)
                ]
            
            print(f"TX command: {tx_cmd}")
            
            try:
                # Test if minimodem works by running a simple command
                test_cmd = [self.minimodem_path, '--version']
                test_result = subprocess.run(test_cmd, capture_output=True, text=True)
                if test_result.returncode != 0:
                    print(f"Error testing minimodem: {test_result.stderr}")
                    return False
                
                # Start the process with error handling
                self.process = subprocess.Popen(
                    tx_cmd,
                    stdin=subprocess.PIPE,
                    stdout=subprocess.DEVNULL,
                    stderr=subprocess.PIPE,
                    bufsize=0,
                    shell=self.shell
                )
                
                # Check if the process started successfully
                if self.process.poll() is not None:
                    # Process failed to start or terminated immediately
                    stderr = self.process.stderr.read().decode('utf-8', errors='ignore')
                    print(f"Error starting TX process: {stderr}")
                    self.process = None
                    return False
                
                # Verify the process is still running after a short delay
                time.sleep(0.2)
                if self.process.poll() is not None:
                    print(f"TX process exited immediately with code {self.process.poll()}")
                    stderr = self.process.stderr.read().decode('utf-8', errors='ignore')
                    print(f"TX process stderr: {stderr}")
                    self.process = None
                    return False
                
                print(f"TX process started with PID {self.process.pid}")
                
                # Send a test message to ensure the process is working
                try:
                    # Send a longer test message with visible output
                    test_message = b"HELLO WORLD THIS IS A TEST MESSAGE\n"
                    self.process.stdin.write(test_message)
                    self.process.stdin.flush()
                    print("Sent test message to minimodem")
                    
                    # Wait a bit to ensure the test message is transmitted
                    time.sleep(2)
                except Exception as e:
                    print(f"Error sending test message: {e}")
                    self.process.terminate()
                    self.process = None
                    return False
                
                # Update the current mode
                self.current_mode = self.MODE_TX
                self.last_mode_switch_time = time.time()
                self.last_tx_time = time.time()  # Initialize last_tx_time
                
                return True
                
            except Exception as e:
                print(f"Exception starting TX process: {e}")
                self.process = None
                return False
    
    def _switch_to_rx_mode(self):
        """Switch to receiver mode. Returns True if successful, False otherwise."""
        with self.mode_switch_lock:
            if self.current_mode == self.MODE_RX and self.process:
                return True  # Already in RX mode
            
            print("Switching to RX mode")
            
            # Terminate the current process if it exists
            if self.process:
                try:
                    self.process.terminate()
                    self.process.wait(timeout=1)
                except:
                    pass
                self.process = None  # Ensure process is cleared
            
            # Start the RX process
            if self.shell:
                rx_cmd = f"{self.minimodem_path} -r --quiet {self.baud_rate}"
            else:
                rx_cmd = [
                    self.minimodem_path,
                    '-r',  # Receive mode
                    '--quiet',
                    str(self.baud_rate)
                ]
            
            print(f"RX command: {rx_cmd}")
            
            try:
                # Test if minimodem works by running a simple command
                test_cmd = [self.minimodem_path, '--version']
                test_result = subprocess.run(test_cmd, capture_output=True, text=True)
                if test_result.returncode != 0:
                    print(f"Error testing minimodem: {test_result.stderr}")
                    return False
                
                # Start the process with error handling
                self.process = subprocess.Popen(
                    rx_cmd,
                    stdin=subprocess.DEVNULL,
                    stdout=subprocess.PIPE,
                    stderr=subprocess.PIPE,
                    bufsize=0,
                    universal_newlines=False,  # Ensure binary mode
                    shell=self.shell
                )
                
                # Check if the process started successfully
                if self.process.poll() is not None:
                    # Process failed to start or terminated immediately
                    stderr = self.process.stderr.read().decode('utf-8', errors='ignore')
                    print(f"Error starting RX process: {stderr}")
                    self.process = None
                    return False
                
                # Verify the process is still running after a short delay
                time.sleep(0.2)
                if self.process.poll() is not None:
                    print(f"RX process exited immediately with code {self.process.poll()}")
                    stderr = self.process.stderr.read().decode('utf-8', errors='ignore')
                    print(f"RX process stderr: {stderr}")
                    self.process = None
                    return False
                
                print(f"RX process started with PID {self.process.pid}")
                
                # Clear the RX buffer
                self.rx_buffer = bytearray()
                
                # Update the current mode
                self.current_mode = self.MODE_RX
                self.last_mode_switch_time = time.time()
                self.last_rx_time = time.time()  # Initialize last_rx_time
                
                return True
                
            except Exception as e:
                print(f"Exception starting RX process: {e}")
                self.process = None
                return False
    
    def _processing_thread(self):
        """Main processing thread that handles mode switching and data processing."""
        print("Processing thread started")
        
        # Initialize last_rx_time and last_tx_time
        self.last_rx_time = time.time()
        self.last_tx_time = time.time()
        
        # Make sure we start in the correct mode
        if self.is_server:
            self._switch_to_rx_mode()
        else:
            self._switch_to_tx_mode()
        
        while self.running:
            try:
                # Handle mode switching based on the current state
                current_time = time.time()
                
                # If we're in TX mode
                if self.current_mode == self.MODE_TX:
                    # Check if there's data to transmit
                    if not self.tx_queue.empty():
                        # Process the data
                        self._process_tx_data()
                        # Stay in TX mode for a bit to allow more data to be queued
                        time.sleep(0.1)
                    else:
                        # No data to transmit, switch to RX mode after a delay
                        if current_time - self.last_tx_time > self.ack_delay:
                            print("No data to transmit, switching to RX mode")
                            self._switch_to_rx_mode()
                
                # If we're in RX mode
                elif self.current_mode == self.MODE_RX:
                    # Process any incoming data
                    data_received = self._process_rx_data()
                    
                    # If we're a transmitter (client) and haven't received data for a while,
                    # switch back to TX mode if there's data to transmit
                    if not self.is_server:
                        if current_time - self.last_rx_time > self.ack_delay:
                            # Check if there's data to transmit
                            if not self.tx_queue.empty():
                                print("No data received, switching to TX mode")
                                self._switch_to_tx_mode()
                    
                    # If we're a receiver (server) and have data to transmit (ACKs/NAKs),
                    # switch to TX mode
                    elif self.is_server and not self.tx_queue.empty():
                        print("Have ACKs/NAKs to send, switching to TX mode")
                        self._switch_to_tx_mode()
                
                # Small delay to avoid busy waiting
                time.sleep(0.1)
            
            except Exception as e:
                print(f"Error in processing thread: {e}")
                # If there was an error, try to restart the current mode
                if self.current_mode == self.MODE_RX:
                    self._switch_to_rx_mode()
                else:
                    self._switch_to_tx_mode()
                time.sleep(1)  # Wait a bit before continuing
        
        print("Processing thread exiting")
    
    def _process_tx_data(self):
        """Process data in the TX queue."""
        try:
            # Check if we're in TX mode
            if self.current_mode != self.MODE_TX:
                print("Not in TX mode, cannot transmit")
                return
            
            # Check if the process is running
            if not self.process or self.process.poll() is not None:
                print("TX process not running, attempting to restart")
                success = self._switch_to_tx_mode()
                if not success:
                    print("Failed to restart TX process, will try again later")
                    time.sleep(1)  # Wait before trying again
                    return
            
            # Get data from the queue (non-blocking)
            try:
                data = self.tx_queue.get_nowait()
            except queue.Empty:
                return
            
            # We have data to send
            print(f"Sending {len(data)} bytes to minimodem")
            self.is_transmitting = True
            
            # Send the data to minimodem
            try:
                # For binary data, convert to a simple text representation
                # Use base64 encoding which is more compact than hex
                encoded_data = base64.b64encode(data)
                
                # Add markers and newlines
                transmission = b"BEGIN_DATA\n" + encoded_data + b"\nEND_DATA\n"
                
                print(f"Sending {len(transmission)} bytes of encoded data to minimodem")
                
                # Write the data in smaller chunks to avoid buffer issues
                chunk_size = 100  # Send 100 bytes at a time
                for i in range(0, len(transmission), chunk_size):
                    chunk = transmission[i:i+chunk_size]
                    self.process.stdin.write(chunk)
                    self.process.stdin.flush()
                    # Small delay between chunks
                    time.sleep(0.1)
                
                # Final newline to ensure transmission
                self.process.stdin.write(b"\n")
                self.process.stdin.flush()
                
                self.last_tx_time = time.time()
                
                # Calculate transmission time based on baud rate
                bytes_per_second = self.baud_rate / 8  # Convert baud to bytes per second
                transmission_time = len(transmission) / bytes_per_second  # Time in seconds
                delay = max(5.0, transmission_time * 2.0)  # Double the expected time as a safety margin
                
                print(f"Transmission will take approximately {transmission_time:.2f} seconds, waiting {delay:.2f} seconds")
                time.sleep(delay)
                
                print("Transmission complete")
            except BrokenPipeError:
                print("TX pipe broken, restarting TX process")
                success = self._switch_to_tx_mode()
                if not success:
                    print("Failed to restart TX process after pipe break")
                # Put the data back in the queue
                self.tx_queue.put(data)
                self.is_transmitting = False
                return
            except Exception as e:
                print(f"Error sending to minimodem: {e}")
                # Put the data back in the queue
                self.tx_queue.put(data)
                self.is_transmitting = False
                return
            
            # Wait for ACKs/NAKs after transmission
            if self.ack_delay > 0:
                print(f"Waiting {self.ack_delay} seconds for ACKs/NAKs...")
                time.sleep(self.ack_delay)
            
            # Clear transmitting flag
            self.is_transmitting = False
            
            # Mark the task as done
            self.tx_queue.task_done()
            
            # Don't switch to RX mode immediately, let the processing thread handle it
            # This allows more data to be transmitted if available
        
        except Exception as e:
            print(f"Error processing TX data: {e}")
            self.is_transmitting = False
    
    def _process_rx_data(self):
        """Process incoming data in RX mode. Returns True if data was received."""
        try:
            # Check if we're in RX mode
            if self.current_mode != self.MODE_RX:
                return False
            
            # Check if the process is running
            if not self.process or self.process.poll() is not None:
                print("RX process not running, attempting to restart")
                success = self._switch_to_rx_mode()
                if not success:
                    print("Failed to restart RX process, will try again later")
                    time.sleep(1)  # Wait before trying again
                return False
            
            # Check if there's data to read
            if self.process.stdout in select.select([self.process.stdout], [], [], 0.1)[0]:
                # Read a chunk of data
                chunk = self.process.stdout.read(1024)
                
                if chunk:
                    # Update last receive time
                    self.last_rx_time = time.time()
                    
                    # Add to buffer
                    self.rx_buffer.extend(chunk)
                    
                    # Look for BEGIN_DATA and END_DATA markers
                    begin_pos = self.rx_buffer.find(b'BEGIN_DATA\n')
                    end_pos = self.rx_buffer.find(b'\nEND_DATA\n')
                    
                    if begin_pos >= 0 and end_pos > begin_pos:
                        # Extract the base64-encoded data
                        encoded_data = self.rx_buffer[begin_pos+11:end_pos]
                        
                        # Remove the processed data from the buffer
                        self.rx_buffer = self.rx_buffer[end_pos+11:]
                        
                        try:
                            # Convert base64 string back to binary
                            binary_data = base64.b64decode(encoded_data)
                            
                            print(f"Received {len(binary_data)} bytes of binary data")
                            
                            # Try to unframe the message to verify it's valid
                            unframed_data = COBSFramer.unframe(binary_data)
                            if unframed_data is not None:
                                # Valid packet, put it in the RX queue
                                self.rx_queue.put(binary_data)
                                print(f"Queued {len(binary_data)} bytes for processing")
                            else:
                                print("Invalid packet, discarding")
                        except Exception as e:
                            print(f"Error processing encoded data: {e}")
                    
                    # If buffer gets too large, clear it
                    if len(self.rx_buffer) > 16384:  # 16KB max buffer
                        print(f"Buffer too large ({len(self.rx_buffer)} bytes), clearing")
                        self.rx_buffer = bytearray()
                    
                    return True  # Data was received
            
            return False  # No data was received
            
        except Exception as e:
            print(f"Error processing RX data: {e}")
            return False


def parse_args():
    """Parse command-line arguments."""
    parser = argparse.ArgumentParser(
        description='Sound Packet Bridge with minimodem'
    )
    
    # Common arguments
    parser.add_argument('--minimodem', default='minimodem',
                        help='Path to minimodem executable (default: minimodem)')
    parser.add_argument('--baud', type=int, default=45,
                        help='Baud rate for minimodem (default: 45)')
    
    # Mode argument (server or client)
    parser.add_argument('mode', choices=['server', 'client'],
                        help='Operating mode: server or client')
    
    args = parser.parse_args()
    return args


def main():
    """Main entry point."""
    args = parse_args()
    
    # Create queues for communication with packet framer
    tx_queue = queue.Queue()  # Data to be sent to minimodem
    rx_queue = queue.Queue()  # Data received from minimodem
    
    # Create signal handler for graceful shutdown
    def signal_handler(sig, frame):
        print("\nInterrupted by user, shutting down...")
        if bridge:
            bridge.stop()
        sys.exit(0)
    
    # Register signal handlers
    signal.signal(signal.SIGINT, signal_handler)
    signal.signal(signal.SIGTERM, signal_handler)
    
    # Create and start the bridge
    bridge = MinimodemBridge(
        minimodem_path=args.minimodem,
        tx_queue=tx_queue,
        rx_queue=rx_queue,
        baud_rate=args.baud,
        is_server=(args.mode == 'server')
    )
    
    try:
        bridge.start()
    except Exception as e:
        print(f"Error: {e}")
        bridge.stop()
        sys.exit(1)


if __name__ == "__main__":
    main()