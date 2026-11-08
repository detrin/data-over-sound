# Data over sound

Two laptops talking through a speaker and a microphone. The main implementation is a single Rust
binary, `dos`:

- **Network card over sound**: an adaptive OFDM modem (~6-9 kbit/s on air) exposed as a Linux
  network interface; ping, UDP and TCP between the machines.
- **File transfer**: the same modem moving files both ways, and the original 100 baud minimodem
  transfer.
- **Research tools**: room measurement, channel simulator, SNR-model calibration and a comparison
  of link-adaptation strategies, in simulation and over the air with broadcast noise.

The project started in Python; that implementation was ported to Rust and removed (it is in the
git history up to commit 35d4ef5). The on-air format is unchanged, and golden vectors recorded
from the Python version keep it byte-identical ([tests](#tests-and-compatibility)). `ax25/` is a
separate, slower IP link over Direwolf and the kernel AX.25 stack.

## Install

Runtime requirements on each machine: Linux, a PipeWire desktop session (`pw-record`,
`pw-play`), the `tun` kernel module and `sudo`; `minimodem` only for `dos modem`.

```bash
git clone https://github.com/detrin/data-over-sound.git && cd data-over-sound
cargo build --release                      # -> target/release/dos
install -m 755 target/release/dos ~/.local/bin/dos
```

A static binary that runs on any x86_64 Linux (including NixOS, no libraries needed) can be built
once and copied to the other machines:

```bash
rustup target add x86_64-unknown-linux-musl   # Debian/Ubuntu also need: sudo apt install musl-tools
cargo build --release --target x86_64-unknown-linux-musl
scp target/x86_64-unknown-linux-musl/release/dos other-machine:.local/bin/dos
```

Audio levels: speaker high, microphone low enough not to clip, e.g.
`wpctl set-volume @DEFAULT_AUDIO_SINK@ 1.0; wpctl set-volume @DEFAULT_AUDIO_SOURCE@ 0.2`.
Don't compile or run heavy jobs on a machine while it is a modem: its fans are noise in the band.

## Network card

```bash
# machine A                                  # machine B
dos net up 10.78.0.1/24                      dos net up 10.78.0.2/24
dos net start leader                         dos net start follower

ping 10.78.0.2                               ping 10.78.0.1
dos net status                               # interface, daemon, last log lines
dos net stop                                 # stop the daemon (the interface stays)
dos net down                                 # stop and delete the interface
```

- `dos net up` (sudo) creates a TUN device `ofdm0` owned by your user, sets the address, MTU 576
  and a route with `rto_min 5s`. `dos net start` runs the daemon unprivileged in your session
  (over ssh too) and logs to `/tmp/dos-net.log`, one line per received frame: tiles decoded,
  levels in use, SNR, queue. `dos net run --role leader` runs it in the foreground.
- One machine is the leader, the other the follower: they take strict turns, so frames never
  collide. Options: `--dev`, `--log`, `--mtu`, `--rto-min`, `--idle-gap`.

**What to expect.** One frame lasts 6.8 s and the machines alternate, so a packet waits up to
~15 s for its slot and a round trip takes ~17-50 s. A frame carries up to ~8 KB when the room is
quiet. It is a long, thin pipe: fine for ping, UDP and patient TCP; interactive ssh works but
each keystroke takes half a minute. Throughput falls with room noise: the band adapts tile by
tile, down to a robust level that carries 10x less.

**Firewalls.** NixOS' default firewall drops new inbound connections on `ofdm0`; start them from
the NixOS side or trust the interface: `networking.firewall.trustedInterfaces = [ "ofdm0" ];`.
It also forgets an unanswered UDP flow after 30 s, shorter than a round trip here, so UDP
request/response from behind it needs keepalives (or the trusted interface).

**Troubleshooting**
- `no answer from follower (timeout)` in the leader's log: the follower is not running or cannot
  hear the leader (volume, distance, `dos net status` on the follower).
- Nothing decodes after the desktop session restarted: the login screen may own the sound card
  (`wpctl status` shows only "Dummy Output"); switch back to your session (`sudo chvt <VT>`).
- Moved the machines or the room changed: the modem adapts on its own; for best results
  re-measure and re-calibrate (`dos measure`, `dos calibrate`, rebuild).

## File transfer and tools

```bash
dos duplex --air --noise --file-a A.bin --file-b B.bin   # two-way adaptive transfer over the air
dos duplex --noise                                        # the same in the simulator
dos modem recv out.bin                                    # minimodem receiver (start first)
dos modem send file.bin                                   # minimodem sender (100 baud, 2300/1900 Hz)
dos check                                                 # PHY loopback + simulated room
dos measure                                               # impulse response + noise, both directions
dos calibrate                                             # SNR -> success model (then rebuild)
dos simulate [--finalists]                                # strategy comparison in the simulator
dos compare --finalists                                   # strategy comparison over the air (~40 min)
```

`measure`, `compare` and `duplex --air` run on a control machine and drive the two laptops over
ssh (`--hosts dell-g5,e540`); the laptops only play and record.

## How it works

**The room.** The impulse response between the laptops is very reverberant: 60 ms after the
direct sound the remaining echo energy is only 10-11 dB down. Short symbols drown in their own
echoes (why AFSK above 100 baud failed). Long OFDM symbols turn most of the reverb into part of
the channel: 640 ms symbols + 160 ms cyclic prefix, 1.5625 Hz subcarrier spacing, 800-10000 Hz,
92 tiles of 100 Hz. Differential PSK in time needs no equaliser. The simulator built from the
measured impulse response and noise reproduces the per-tile SNR over the air within 1-2 dB.

**Frames.** Sync chirp, reference symbol, a DBPSK header on every subcarrier (two soft-combined
copies, RS(255,64), byte-contiguous interleaving against narrowband noise) and six data symbols.
Each tile carries one level: pilot, DBPSK with 3x repetition (robust fallback), DQPSK or D8PSK,
and its own RS+CRC block, so it succeeds or fails on its own. The header gives an SNR for every
tile in every frame: free exploration of the whole band.

**Adaptation.** The receiver of each direction picks the next tile map and sends it back in its
own header with an ACK bitmap; there is no side channel. Strategies were compared under a
changing-noise scenario (quiet, jammer 2-4 kHz, jammer 6-9 kHz, broadband noise, 1-5 kHz bursts,
quiet), over the air with the receiving laptop broadcasting the noise from its own speaker:

| strategy | sim (Python version) | sim (Rust) | air, paired vs fixed D8PSK |
|---|---|---|---|
| Thompson sampling + SNR prior (gamma 0.7) | 5.58 kbit/s | 5.54 | **+584 +/- 173** / -130 +/- 221 bit/s |
| ... + online SNR-model calibration | 5.55 | 5.50 | +494 +/- 229 / -196 +/- 311 |
| SNR greedy (max expected throughput) | 5.40 | 5.45 | |
| fixed D8PSK | 5.25 | 5.23 | baseline |
| Minstrel (EWMA + 10% lookaround) | 5.01 | 4.99 | |
| OLLA (outer-loop margin, 10% BLER) | 3.95 | 3.94 | |

Thompson sampling with the calibrated SNR model as prior and discounted outcome counts is used
for file transfer. For the network card (Rust) it is made risk-averse (maximise
`capacity * p^2`): an IP packet spans several tiles and needs all of them, so tile reliability is
worth more than raw tile throughput (-2% throughput, +4 points tile success in simulation).

## Tests and compatibility

`dos` keeps the on-air format of the original Python implementation; the pseudo-random tables that
define it (reference phases, pilots, header scattering) were drawn with numpy's generator and
live in `data/wire_v1.bin`, which `dos` embeds. Before the Python code was removed, the two were
tested against each other in every combination below.

- `cargo test`: Reed-Solomon codewords and corrections identical to `reedsolo`; header packing,
  sync chirp and minimodem framing (CRC32, RS, COBS, base64 markers) identical to the Python
  version; frames recorded from it decode byte-exact (`tests/golden/`); loopback, file selective
  repeat, IP fragmentation and a two-node exchange over the simulated room.
- The Python decoder decoded frames written by `dos interop-gen` byte-exact.
- Simulation: the strategy comparison gives the same ranking and numbers within noise
  (table above).
- Over the air, dell-g5 and e540, 2026-11-08 (a VMware VM was running on dell-g5 and its fans
  added ~4 dB of noise at that end):

  | link | result |
  |---|---|
  | Rust leader + Rust follower | ping both ways 0% loss (RTT 17-26 s); UDP request/reply both ways; TCP: 81,628 B one way and 15,576 B the other, both SHA-256 identical, in 368 s |
  | Rust leader + Python follower | ping both ways 0% loss; TCP both files identical (1451 s; the Python side uses its MTU-1200-era settings) |
  | Python leader + Rust follower | ping 6/6 both ways |
  | minimodem: Rust -> Python, Python -> Rust | 209 bytes identical each way, 122 s (13.7 bit/s) |
  | `dos measure` | same reverb figures as the Python measurement (-4 dB beyond 20 ms, -11 dB beyond 60 ms) |
  | `dos duplex --air --noise` (from a control machine; six broadcast noise phases) | 81,628 B and 11,191 B both ways, identical, in 26 rounds |

Differences from the Python version (the on-air format is unchanged): one static binary with the
SNR model embedded; `dos net up/start/stop/status/down` replaced a shell script; the network card
uses a risk-averse controller, MTU 576, up to 6 fragment retries and better idle detection
(round trips 17-26 s instead of 17-50 s); the simulator runs strategies in parallel; frames
decode in ~0.1 s on the laptops.

## minimodem file transfer

`dos modem send/recv` drives the external `minimodem` tool at 100 baud. Mark/space must sit
where the speaker-to-microphone path is flat: laptop speakers are weak below ~1 kHz, and
minimodem's default 1270/1070 Hz pair arrived with the 1070 Hz tone 15 dB down, which made every
decode fail. 2300/1900 Hz (the default here) worked in both directions, ~13 bit/s with ARQ.

## IP over AX.25 (`ax25/`)

A slower network card through Direwolf (AFSK + FX.25 FEC, carrier-sense half-duplex) and the
kernel's AX.25 stack via `kissattach`:

```
# on dell-g5                                   # on e540
ax25/up.sh DELL 10.77.0.1 E540 10.77.0.2       ax25/up.sh E540 10.77.0.2 DELL 10.77.0.1
ping 10.77.0.2                                 ping 10.77.0.1
ax25/down.sh                                   ax25/down.sh
```

Needs `direwolf`, `ax25-tools`, the `ax25`/`mkiss` modules and a desktop session. Only 100 baud
on 2300/1900 Hz decodes in that room (1200 and 300 baud: nothing). Ping RTT ~25-40 s, UDP ~30 s,
TCP ~200 bytes each way in ~9 minutes. TCP needed a longer SYN retry budget (`tcp-syncnt=12`),
egress policers that drop repeated SYN/SYN-ACKs, route `rto_min 30s`, MTU 200, and wide CSMA
backoff in Direwolf (`SLOTTIME 100`, `PERSIST 32`), because both ends back off identically and
their retransmits collided on air.
