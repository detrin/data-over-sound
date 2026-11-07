#!/usr/bin/env bash
# up.sh CALL IP PEER_CALL PEER_IP — create ax0: IPv4 over AX.25 over the sound card (Direwolf).
# Run as the desktop user (Direwolf needs the PipeWire session); uses sudo for the kernel side.
set -euo pipefail
CALL=$1 IP=$2 PEER_CALL=$3 PEER_IP=$4
NET=${NET:-10.77.0.0/24} TXDELAY=${TXDELAY:-60} RTO_MIN=${RTO_MIN:-30s}
# 100 baud on 2300/1900 Hz: the fastest rate that survived these laptops' speaker->room->mic path
MODEM=${MODEM:-"100 2300:1900"}
# Frames must fit one FX.25 RS(255,239) block (239 B: AX.25 header 16 + IP + CRC 2) to keep FEC
MTU=${MTU:-200}
# Egress policers (drop, never queue). TCP's 1 s initial RTO vs a ~25 s RTT otherwise floods the
# half-duplex channel with SYN / SYN-ACK retransmits and nobody gets to listen:
#   SYN_RATE: TCP segments with SYN set (SYN, SYN-ACK) - first one passes, repeats for ~60 s dropped
#   RATE:     everything else, ~half the air rate with a 3-packet burst
SYN_RATE=${SYN_RATE:-10bit} RATE=${RATE:-40bit}
DIR=$(cd "$(dirname "$0")" && pwd)

# Kernel link-layer address of an AX.25 callsign (CALL or CALL-SSID): 6 shifted ASCII chars + SSID byte
ax25_lladdr() {
  local base=${1%%-*} ssid=0 out="" i c
  [[ $1 == *-* ]] && ssid=${1#*-}
  base=$(printf '%-6s' "${base^^}")
  for ((i = 0; i < 6; i++)); do printf -v c '%d' "'${base:i:1}"; out+=$(printf '%02x:' $((c << 1))); done
  printf '%s%02x\n' "$out" $((0x60 | (ssid & 15) << 1))
}
RUN=/tmp/ax25snd
export XDG_RUNTIME_DIR=/run/user/$(id -u)

"$DIR/down.sh" >/dev/null 2>&1 || true
mkdir -p "$RUN"
sed -e "s/@CALL@/$CALL/" -e "s/@TXDELAY@/$TXDELAY/" -e "s/@MODEM@/$MODEM/" "$DIR/direwolf.conf" > "$RUN/direwolf.conf"
nohup direwolf -t 0 -p -c "$RUN/direwolf.conf" > "$RUN/direwolf.log" 2>&1 &
for _ in $(seq 50); do [ -e /tmp/kisstnc ] && break; sleep 0.2; done
[ -e /tmp/kisstnc ] || { echo "direwolf did not create /tmp/kisstnc"; tail "$RUN/direwolf.log"; exit 1; }

sudo modprobe mkiss
sudo mkdir -p /etc/ax25
# axports speed column is the pty line speed (unused); the air rate is set by MODEM
echo "snd $CALL 9600 255 2 sound card AFSK link" | sudo tee /etc/ax25/axports >/dev/null
sudo "$(command -v kissattach)" "$(readlink -f /tmp/kisstnc)" snd "$IP" >/dev/null
DEV=ax0
for _ in $(seq 25); do ip link show "$DEV" >/dev/null 2>&1 && break; sleep 0.2; done
# plain KISS: stop mkiss from using its SMACK/FlexNet CRC variants (Direwolf rejects their channel bits)
sudo "$(command -v kissparms)" -c 1 -p snd

# kissattach assigns a classful /8 address + route; narrow both to our link net
sudo ip addr replace "$IP/${NET#*/}" dev "$DEV"
sudo ip addr del "$IP/8" dev "$DEV" 2>/dev/null || true
# kissattach adds a classful 10.0.0.0/8 route; replace it with just our link net
sudo ip route del 10.0.0.0/8 dev "$DEV" 2>/dev/null || true
sudo ip route replace "$NET" dev "$DEV" rto_min "$RTO_MIN"
sudo ip link set "$DEV" mtu "$MTU"
sudo tc qdisc replace dev "$DEV" root handle 1: prio
# IPv4 + TCP + SYN flag (byte 33 = TCP flags, assuming no IP options)
sudo tc filter replace dev "$DEV" parent 1: protocol ip prio 1 u32 \
  match ip protocol 6 0xff match u8 0x02 0x02 at 33 \
  action police rate "$SYN_RATE" burst 80 conform-exceed drop/ok
sudo tc filter replace dev "$DEV" parent 1: protocol all prio 2 matchall \
  action police rate "$RATE" burst $((3 * MTU)) conform-exceed drop/ok
# static neighbour: no ARP broadcasts over the air
sudo ip neigh replace "$PEER_IP" lladdr "$(ax25_lladdr "$PEER_CALL")" dev "$DEV" nud permanent
ip -br addr show "$DEV"; ip route show dev "$DEV"; ip neigh show dev "$DEV"; tc filter show dev "$DEV" parent 1: | grep -E "police|rate"
