# Data over sound

Two laptops talking through a speaker and a microphone, three ways:

1. **Network card over sound (OFDM)**: `ofdm/` — an adaptive OFDM modem (~6-9 kbit/s) exposed as
   a Linux network interface; ping, UDP and TCP between the machines. Start here:
   [install and use](#network-card-over-sound-ofdm--install-and-use).
2. **IP over AX.25** (`ax25/`): Direwolf soft-modem + the kernel packet-radio stack, 100 baud.
3. **File transfer with minimodem** (`main.py`): the original proof of concept, now working.

The rest of this README describes each, the measurements behind the design and how to reproduce
them.

## Network card over sound (OFDM) — install and use

`ofdm/netd.py` turns the adaptive OFDM modem into a Linux network interface (`ofdm0`): ping,
UDP and TCP run over the speaker and microphone between two machines, with the band adapting
to noise as it changes. The modem runs on each machine itself; nothing else is needed.

**Requirements (both machines):** Linux with a PipeWire desktop session (`pw-record`/`pw-play`),
the `tun` kernel module, `sudo`, Python 3 with `numpy`, `scipy` and `reedsolo`, and a speaker and a
microphone that can hear each other (we used two laptops ~1-2 m apart).

### Install

```bash
git clone https://github.com/detrin/data-over-sound.git ~/data-over-sound

# Ubuntu / Debian
sudo apt install python3-numpy python3-scipy python3-reedsolo pipewire-bin

# NixOS (no system Python needed): build a Python with the three packages
nix build --impure -o ~/.local/nix/pyofdm --expr \
  '(builtins.getFlake "nixpkgs").legacyPackages.x86_64-linux.python3.withPackages (p: [p.numpy p.scipy p.reedsolo])'
export PYTHON=~/.local/nix/pyofdm/bin/python3        # used by ofdmnet.sh
```

Audio: set the speaker volume high and the microphone low enough not to clip, e.g.
`wpctl set-volume @DEFAULT_AUDIO_SINK@ 1.0; wpctl set-volume @DEFAULT_AUDIO_SOURCE@ 0.3`.

### Use

```bash
# machine A (leader)                                   # machine B (follower)
~/data-over-sound/ofdm/ofdmnet.sh up 10.78.0.1/24      ~/data-over-sound/ofdm/ofdmnet.sh up 10.78.0.2/24
~/data-over-sound/ofdm/ofdmnet.sh start leader         ~/data-over-sound/ofdm/ofdmnet.sh start follower

ping 10.78.0.2                                         ping 10.78.0.1
~/data-over-sound/ofdm/ofdmnet.sh status               # interface, daemon, last log lines
~/data-over-sound/ofdm/ofdmnet.sh stop                 # stop the daemon (interface stays)
~/data-over-sound/ofdm/ofdmnet.sh down                 # stop and delete the interface
```

- `up` (sudo) creates a TUN device owned by your user, sets the address, MTU 1200 and a route
  with `rto_min 5s`; `start` runs the daemon unprivileged in your session and logs to
  `/tmp/ofdmnet.log` (one line per received frame: tiles decoded, levels in use, SNR, queue).
- Start it from the logged-in desktop user (over ssh is fine: it sets `XDG_RUNTIME_DIR`). One
  machine must be the leader, the other the follower; the leader starts the exchange.
- Environment overrides: `DEV`, `MTU`, `RTO_MIN`, `PYTHON`, `LOG`.

**Measured between dell-g5 (leader) and e540 (follower), 2026-11-07:** ping both ways 0% loss
(RTT 18-50 s); UDP request/reply both ways (46 s); one TCP connection moved an 81,628-byte JPEG
one way and a 10,751-byte file the other way, both SHA-256 identical, in 312 s (~2.4 kbit/s of
TCP goodput including handshake and slow start). The maps settle at mostly D8PSK with ~80-90 of
92 tiles decoding per frame in a quiet room.

**What to expect.** One frame is 6.8 s; the two machines take turns, so a packet waits up to
~15 s for its slot and a round trip takes ~20-50 s. Each frame carries up to ~8 KB (6-9 kbit/s
per direction when the room is quiet). It is a very long, thin pipe: fine for ping, UDP, file
transfer and patient TCP; interactive use (ssh) works but each keystroke takes half a minute.

**Firewalls.** Packets on `ofdm0` look like any other interface. NixOS' default firewall drops
new inbound connections, so connections must be started from the NixOS side unless you trust the
interface: `networking.firewall.trustedInterfaces = [ "ofdm0" ];`. It also forgets an unanswered
UDP flow after 30 s, shorter than one round trip here, so UDP request/response from behind it
needs keepalives (or the trusted interface).

**Troubleshooting**
- Log says `no answer from follower`: the follower is not running, or cannot hear the leader
  (volume, distance, `ofdmnet.sh status` on the follower).
- Nothing decodes after the machine's desktop session restarted: the login screen may own the
  sound card (`wpctl status` shows only "Dummy Output"); switch back to your session
  (`sudo chvt <your VT>`) or log in again.
- Moved the machines? The modem adapts on its own; for best results re-run the room measurement
  and calibration (`uv run python -m ofdm.measure_channel && uv run python -m ofdm.calibrate`).

## Overview (minimodem file transfer)

A modular sound modem that transmits digital data over audio using MFSK (Multi-Frequency Shift Keying) modulation. It uses the following components:

1. **packet_framer.py**: Handles packet framing, error correction, and reliable transmission
2. **sound_packet_bridge.py**: Bridges the packet framer with minimodem for audio transmission
3. **minimodem**: External tool for MFSK modulation/demodulation
4. **main.py**: Integrates all components for easy use

The system supports:
- COBS (Consistent Overhead Byte Stuffing) for packet framing
- Reed-Solomon forward error correction
- CRC32 integrity check on every frame
- Stop-and-Wait ARQ for reliable transmission (the final EOT is ACKed too)
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

### Usage

Start the receiver first, then the transmitter. Both sides must use the same modem options.

```
# Receiver
python main.py --baud 100 --mark 2300 --space 1900 --fec --arq rx output.txt

# Transmitter
python main.py --baud 100 --mark 2300 --space 1900 --fec --arq tx input.txt
```

Pick `--mark`/`--space` where the speaker-to-microphone path is flat: laptop speakers are weak
below ~1 kHz, and minimodem's default 1270/1070 Hz pair arrived with the 1070 Hz tone 15 dB down,
which made every decode fail. 2300/1900 Hz at 100 baud worked in both directions between two
laptops (dell-g5 <-> e540), ~10-15 bit/s effective with ARQ. Keep the microphone gain low
enough that the recording does not clip.

## Adaptive OFDM modem (`ofdm/`): ~6-9 kbit/s, adapts to changing noise

A multi-carrier modem tuned on the real room between the two laptops, with per-tile link
adaptation. All DSP runs on the Mac; the laptops only play and record (`ofdm/air.py`).

| Module | Role |
|---|---|
| `measure_channel.py` | exponential-sweep impulse response + noise PSD per direction -> `data/channel_*.npz` |
| `channel.py` | channel simulator calibrated from those measurements (+ time-varying interferers) |
| `phy.py` | OFDM PHY: sync chirp, reference, DBPSK header, data tiles, differential PSK in time |
| `frame.py` | per-tile blocks (payload, CRC-16, RS) and the header with feedback |
| `calibrate.py` | header-SNR -> block-success model per level (`data/snr_model.json`) |
| `controllers.py` | link-adaptation strategies (fixed, SNR threshold, OLLA, Minstrel, Thompson sampling) |
| `simulate.py` / `air_compare.py` | strategy comparison in simulation / over the air with broadcast noise |
| `duplex.py` | the two-way link: TDD, in-band feedback, selective-repeat ARQ, adaptive maps |

**What the room dictated.** The impulse response is very reverberant: even 60 ms after the direct
sound the remaining echo energy is only 7-12 dB down. Short symbols drown in their own echoes
(why AFSK above 100 baud failed). Long OFDM symbols turn most of the reverb into part of the
channel: 640 ms symbols + 160 ms cyclic prefix, 1.5625 Hz spacing, 800-10000 Hz, 92 tiles of
100 Hz. Differential PSK in time needs no equaliser. The simulator reproduces the measured
per-tile SNR over the air within 1-2 dB.

**Levels per tile:** 0 = pilot (measured, no data), 1 = DBPSK x3 repetition (robust fallback),
2 = DQPSK, 3 = D8PSK. Each tile is its own RS+CRC block, so it succeeds or fails on its own.
The header (DBPSK on every subcarrier, two soft-combined copies, RS(255,64), byte-contiguous
interleaving) gives an SNR for **every** tile in every frame: free exploration of the whole band.

**Strategies compared** under a changing-noise scenario (`scenario.py`: quiet, jammer 2-4 kHz,
jammer 6-9 kHz, broadband noise, 1-5 kHz bursts, quiet). Over the air the receiving laptop
broadcasts the noise from its own speaker while it records.

| | sim mean | air round 1 (D->E / E->D) | air round 2, paired vs fixed-L3 |
|---|---|---|---|
| Thompson sampling + SNR prior (gamma 0.7) | 5.58 kbit/s | 5.41 / 5.65 | **+584 +/- 173** / -130 +/- 221 bit/s |
| ... + online SNR-model calibration | 5.55 | - | +494 +/- 229 / -196 +/- 311 |
| SNR greedy (max expected throughput) | 5.40 | 5.79 / 5.12 | - |
| fixed D8PSK | 5.25 | 5.41 / 5.72 | baseline |
| Minstrel (EWMA + 10% lookaround) | 5.01 | 5.75 / 5.15 | - |
| OLLA (outer-loop margin, 10% BLER) | 3.95 | - | - |

Thompson sampling with the calibrated SNR model as prior and discounted outcome counts is the
chosen controller: best in simulation, significantly better than fixed D8PSK in one air
direction (+11%) and tied in the other. Minstrel and OLLA recover slowly after a jammer leaves;
the room is good enough that fixed D8PSK is near-optimal when quiet, so the gains come in the
noisy phases. Flat broadband noise at +8 dB SIR (most tiles below ~4 dB) breaks every strategy.

**Two-way result over the air** (`duplex.py --air --noise`, all six noise phases broadcast, 3
rounds each): dell-g5 sent the 81,628-byte JPEG while e540 sent this README (6,601 bytes); both
arrived byte-identical in 22 rounds. The image direction moved 81.6 KB in 22 frames (~150 s of
its airtime, ~4.4 kbit/s) through quiet, jammed, broadband and burst phases, with the maps
travelling only in the in-band headers.

```
uv run python -m ofdm.measure_channel           # re-measure the room (laptops on, desktop session active)
uv run python -m ofdm.calibrate                 # SNR -> success model
uv run python -m ofdm.simulate                  # strategy comparison in simulation
uv run python -m ofdm.air_compare --finalists   # over the air, with broadcast noise (~40 min)
uv run python -m ofdm.duplex --air --noise      # two-way adaptive transfer over the air
uv run python -m ofdm.test_ofdm                 # offline regression tests
```

## IP over sound (virtual network card)

`ax25/` turns the sound card into a Linux network interface (`ax0`) so ordinary IP, UDP and
TCP run over the air. It uses Direwolf as a software modem (AFSK + FX.25 Reed-Solomon FEC,
carrier-sense half-duplex) and the kernel's AX.25 stack via `kissattach`.

```
# on dell-g5                                   # on e540
ax25/up.sh DELL 10.77.0.1 E540 10.77.0.2       ax25/up.sh E540 10.77.0.2 DELL 10.77.0.1
ping 10.77.0.2                                 ping 10.77.0.1
ax25/down.sh                                   ax25/down.sh
```

Needs `direwolf`, `ax25-tools` (`kissattach`, `kissparms`), the `ax25`/`mkiss` kernel modules and
a desktop session (Direwolf records/plays through PipeWire's ALSA plugin). Tunables are env vars:
`MODEM` (default `100 2300:1900`, i.e. 100 baud, mark 2300 Hz, space 1900 Hz), `MTU` (200, so a
frame fits one FX.25 RS(255,239) block), `RATE` (tbf rate on `ax0`, ~air rate with a one-packet
queue so TCP retransmits are dropped instead of piling up behind the frame on air), `TXDELAY`,
`RTO_MIN` (route `rto_min`, default 30s).

What we measured between the two laptops: 1200 and 300 baud decoded nothing over the air;
100 baud works with FX.25 correcting a few bytes per frame now and then. Ping RTT ~25-40 s, a
UDP request/reply ~30 s, and one TCP connection moved ~200 bytes each way (byte-identical) in
~9 minutes. What TCP needed on this half-duplex ~25 s-RTT channel:

- a longer SYN retry budget than the default 6 (`socat ... TCP4:10.77.0.1:6000,tcp-syncnt=12`),
  because the first SYN timeout is 1 s while a SYN takes ~13 s on air;
- egress policers that drop repeated SYN/SYN-ACKs and cap the rest at ~half the air rate, so
  retransmits don't keep the transmitter busy and the peer gets heard;
- route `rto_min 30s`, MTU 200 (one FX.25 block per frame);
- wide CSMA backoff in Direwolf (`SLOTTIME 100`, `PERSIST 32`): both ends back off identically,
  and with the defaults their retransmits collided on air every time.

The NixOS firewall drops new inbound connections on `ax0` (it trusts only `lo`/`tailscale0`),
so on NixOS either start connections from that host or add `ax0` to
`networking.firewall.trustedInterfaces`.

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
- `--baud RATE`: Baud rate for minimodem (default: 100)
- `--mark HZ` / `--space HZ`: tone frequencies (default: minimodem's)
- `--turnaround SEC`: pause before transmitting so the peer can switch to RX (default: 1.0)
- `--payload-size SIZE`: Maximum payload size in bytes (default: 64)
- `--fec`: Enable Reed-Solomon forward error correction
- `--fec-symbols N`: Number of Reed-Solomon error correction symbols (default: 16)
- `--arq`: Enable Selective Repeat ARQ for reliable transmission
- `--timeout SEC`: ARQ ACK timeout (default: computed from airtime)

## Examples

### Simple File Transfer

```
# Receiver
python main.py --baud 100 --mark 2300 --space 1900 rx output.txt

# Transmitter
python main.py --baud 100 --mark 2300 --space 1900 tx input.txt
```

### Reliable Transfer with Error Correction

For more reliable transmission with error correction and automatic retransmission:

```
# Receiver
python main.py --baud 100 --mark 2300 --space 1900 --fec --fec-symbols 64 --arq rx output.txt

# Transmitter
python main.py --baud 100 --mark 2300 --space 1900 --fec --fec-symbols 64 --arq tx input.txt
```

## Troubleshooting

- **No data transmission**: Ensure audio levels are appropriate and there's no background noise
- **Corrupted data**: Try reducing the baud rate or increasing FEC symbols
- **Slow transmission**: Increase the baud rate or payload size, but be aware this may reduce reliability

Gotcha (dell-g5): if the desktop session restarts and the GDM greeter becomes the active seat
session, logind hands the sound card to gdm's PipeWire and Direwolf silently plays into
"Dummy Output". `sudo chvt <vt of the user session>` gives it back.
