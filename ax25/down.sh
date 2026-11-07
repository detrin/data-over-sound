#!/usr/bin/env bash
# down.sh — tear down ax0 and Direwolf.
sudo pkill -x kissattach || true
pkill -x direwolf || true
sleep 1
rm -f /tmp/kisstnc
