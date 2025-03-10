# Sound Modem

A modular sound modem that transmits digital data over audio using MFSK (Multi-Frequency Shift Keying) modulation.

## Overview

This project provides a complete system for transmitting digital data over audio channels. It uses the following components:

1. **packet_framer.py**: Handles packet framing, error correction, and reliable transmission
2. **sound_packet_bridge.py**: Bridges the packet framer with minimodem for audio transmission
3. **minimodem**: External tool for MFSK modulation/demodulation
4. **main.py**: Integrates all components for easy use

The system supports:
- COBS (Consistent Overhead Byte Stuffing) for packet framing
- Reed-Solomon forward error correction
- Selective Repeat ARQ for reliable transmission
- Queue-based communication between components

## Requirements

- Python 3.6+
- minimodem (https://github.com/kamalmostafa/minimodem)
- reedsolo Python package (optional, for FEC)

## Installation

1. Install minimodem:
   ```
   # On Ubuntu/Debian
   sudo apt-get install minimodem
   
   # On macOS with Homebrew
   brew install minimodem
   ```

2. Install Python dependencies:
   ```
   pip install reedsolo
   ```

## Usage

### Using main.py (Recommended)

The simplest way to use the system is through main.py, which integrates all components:

```
# Transmitter mode
python main.py --minimodem /Users/hermanda/Documents/projects/temp/minimodem/src/minimodem --baud 45 --fec --arq tx input.txt

# Receiver mode
python main.py --minimodem /Users/hermanda/Documents/projects/temp/minimodem/src/minimodem --baud 45 --fec --arq rx output.txt
```

### Using Components Separately

Alternatively, you can run the components separately:

#### 1. Sound Packet Bridge

This component interfaces with minimodem to send and receive data over audio.

```
# Server mode (receiver)
python sound_packet_bridge.py --minimodem /path/to/minimodem --baud 45 server

# Client mode (transmitter)
python sound_packet_bridge.py --minimodem /path/to/minimodem --baud 45 client
```

#### 2. Packet Framer

This component handles the actual data transmission and reception.

```
# Transmitter mode
python packet_framer.py --payload-size 128 --fec --fec-symbols 32 --arq --window-size 4 --timeout 10.0 tx input.txt

# Receiver mode
python packet_framer.py --payload-size 128 --fec --fec-symbols 32 --arq --window-size 4 --timeout 10.0 rx output.txt
```

## Architecture

The system uses a queue-based architecture:

1. **packet_framer.py** prepares data for transmission by:
   - Splitting it into packets
   - Adding error correction (optional)
   - Framing with COBS
   - Placing framed packets in a queue

2. **sound_packet_bridge.py**:
   - Takes framed packets from the queue
   - Sends them to minimodem for audio transmission
   - Receives audio data from minimodem
   - Validates and parses received packets
   - Places valid packets in a queue for processing

3. **minimodem** handles the actual audio modulation/demodulation

4. **main.py** integrates all components and provides a simple interface

## Command-Line Options

### Main Options

- `--minimodem PATH`: Path to minimodem executable (default: 'minimodem')
- `--baud RATE`: Baud rate for minimodem (default: 45)
- `--payload-size SIZE`: Maximum payload size in bytes (default: 128)
- `--fec`: Enable Reed-Solomon forward error correction
- `--fec-symbols N`: Number of Reed-Solomon error correction symbols (default: 32)
- `--arq`: Enable Selective Repeat ARQ for reliable transmission
- `--window-size N`: ARQ window size (default: 4)
- `--timeout SEC`: ARQ timeout in seconds (default: 10.0)

## Examples

### Simple File Transfer

```
# Receiver
python main.py --baud 45 rx output.txt

# Transmitter
python main.py --baud 45 tx input.txt
```

### Reliable Transfer with Error Correction

For more reliable transmission with error correction and automatic retransmission:

```
# Receiver
python main.py --baud 45 --fec --fec-symbols 64 --arq --window-size 4 rx output.txt

# Transmitter
python main.py --baud 45 --fec --fec-symbols 64 --arq --window-size 4 tx input.txt
```

## Troubleshooting

- **No data transmission**: Ensure audio levels are appropriate and there's no background noise
- **Corrupted data**: Try reducing the baud rate or increasing FEC symbols
- **Slow transmission**: Increase the baud rate or payload size, but be aware this may reduce reliability
