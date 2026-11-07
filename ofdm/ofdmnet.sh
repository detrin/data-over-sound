#!/usr/bin/env bash
# ofdmnet.sh — the adaptive OFDM acoustic modem as a network card.
#   ofdmnet.sh up IP/PREFIX         create user-owned TUN $DEV, address, MTU, TCP-friendly route (sudo)
#   ofdmnet.sh start leader|follower run the daemon in the background (desktop user, PipeWire session)
#   ofdmnet.sh stop                 stop the daemon
#   ofdmnet.sh status               interface, daemon and the last log lines
#   ofdmnet.sh down                 stop and delete the interface (sudo)
# Env: DEV=ofdm0 MTU=1200 RTO_MIN=5s PYTHON=python3 LOG=/tmp/ofdmnet.log
set -euo pipefail
DEV=${DEV:-ofdm0} MTU=${MTU:-1200} RTO_MIN=${RTO_MIN:-5s}
PYTHON=${PYTHON:-python3} LOG=${LOG:-/tmp/ofdmnet.log} PIDFILE=/tmp/ofdmnet-$DEV.pid
ROOT=$(cd "$(dirname "$0")/.." && pwd)          # directory that contains the ofdm package

stop() {
  if [ -f "$PIDFILE" ] && kill "$(cat "$PIDFILE")" 2>/dev/null; then echo "stopped"; fi
  rm -f "$PIDFILE"
}

case "${1:-}" in
  up)
    IP=${2:?usage: ofdmnet.sh up IP/PREFIX}
    sudo ip tuntap add dev "$DEV" mode tun user "$(id -un)" 2>/dev/null || true
    sudo ip addr replace "$IP" dev "$DEV"
    sudo ip link set "$DEV" mtu "$MTU" up
    # one exchange takes ~15-20 s: keep TCP from retransmitting every second
    NET=$(ip -4 route show dev "$DEV" proto kernel | awk 'NR==1{print $1}')
    sudo ip route replace "$NET" dev "$DEV" proto kernel scope link rto_min "$RTO_MIN"
    ip -br addr show dev "$DEV"; ip route show dev "$DEV" ;;
  start)
    ROLE=${2:?usage: ofdmnet.sh start leader|follower}
    stop >/dev/null
    export XDG_RUNTIME_DIR=${XDG_RUNTIME_DIR:-/run/user/$(id -u)}
    cd "$ROOT"
    nohup "$PYTHON" -u -m ofdm.netd --role "$ROLE" --dev "$DEV" > "$LOG" 2>&1 &
    echo $! > "$PIDFILE"; sleep 2; tail -3 "$LOG" ;;
  stop) stop ;;
  status)
    ip -br addr show dev "$DEV" 2>/dev/null || echo "$DEV: not present"
    if [ -f "$PIDFILE" ] && kill -0 "$(cat "$PIDFILE")" 2>/dev/null; then echo "daemon running (pid $(cat "$PIDFILE"))"; else echo "daemon not running"; fi
    tail -5 "$LOG" 2>/dev/null || true ;;
  down) stop; sudo ip link del "$DEV" 2>/dev/null || true ;;
  *) sed -n '2,8p' "$0"; exit 1 ;;
esac
