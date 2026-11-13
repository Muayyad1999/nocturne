#!/usr/bin/env bash
# Runs the routed-network tests (tests/lab.rs) inside a throw-away topology of Linux network
# namespaces. Needs root (or CAP_NET_ADMIN + CAP_SYS_ADMIN) and iproute2.
#
#   client ── r1 ── r2 ── target
#
#   client  10.201.1.1/24        (ping sockets enabled, so both ICMP socket kinds are testable)
#   r1      10.201.1.254 and .50 on the client LAN, ignores echo requests (answers ARP and TCP)
#           10.201.2.1/24 towards r2; no route to 10.201.9.0/24 (-> network unreachable)
#   r2      10.201.2.254/24, 10.201.3.254/24; rejects 10.201.66.0/24 (-> admin prohibited)
#   target  10.201.3.10/24
#
# Usage: tests/lab/run.sh [extra arguments for the test binary]
set -euo pipefail

cd "$(dirname "$0")/../.."
crate_dir=$PWD
export CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-/home/user/.cache/nocturne-target/netprobe}

# The integration tests of tests/lab.rs, and the unit tests named lab_* (the ones that need
# crate-internal access, such as driving the system traceroute directly).
built=$(cd ../.. && cargo test -p nocturne-netprobe --lib --test lab --no-run --message-format=json 2>/dev/null \
    | grep -o '"executable":"[^"]*"' | cut -d'"' -f4)
binary=$(echo "$built" | grep '/lab-' | tail -1)
unit_binary=$(echo "$built" | grep '/nocturne_netprobe-' | tail -1)
[ -x "$binary" ] && [ -x "$unit_binary" ] || { echo "could not build the lab tests" >&2; exit 1; }

p="npl$$"
c="$p-c"; r1="$p-r1"; r2="$p-r2"; t="$p-t"
listener_pid=""
cleanup() {
    [ -n "$listener_pid" ] && kill "$listener_pid" 2>/dev/null || true
    for ns in "$c" "$r1" "$r2" "$t"; do ip netns del "$ns" 2>/dev/null || true; done
}
trap cleanup EXIT

for ns in "$c" "$r1" "$r2" "$t"; do
    ip netns add "$ns"
    ip -n "$ns" link set lo up
    # Deterministic tests: no ICMP rate limiting inside the lab.
    ip netns exec "$ns" sysctl -qw net.ipv4.icmp_ratelimit=0
done

ip -n "$c" link add lan type veth peer name lan netns "$r1"
ip -n "$r1" link add core type veth peer name core netns "$r2"
ip -n "$r2" link add edge type veth peer name edge netns "$t"

ip -n "$c" addr add 10.201.1.1/24 dev lan
ip -n "$r1" addr add 10.201.1.254/24 dev lan
ip -n "$r1" addr add 10.201.1.50/24 dev lan
ip -n "$r1" addr add 10.201.2.1/24 dev core
ip -n "$r2" addr add 10.201.2.254/24 dev core
ip -n "$r2" addr add 10.201.3.254/24 dev edge
ip -n "$t" addr add 10.201.3.10/24 dev edge
ip -n "$c" link set lan up; ip -n "$r1" link set lan up
ip -n "$r1" link set core up; ip -n "$r2" link set core up
ip -n "$r2" link set edge up; ip -n "$t" link set edge up

ip -n "$c" route add default via 10.201.1.254
ip -n "$t" route add default via 10.201.3.254
ip -n "$r1" route add 10.201.3.0/24 via 10.201.2.254
ip -n "$r1" route add 10.201.66.0/24 via 10.201.2.254
ip -n "$r2" route add 10.201.1.0/24 via 10.201.2.1
ip -n "$r2" route add prohibit 10.201.66.0/24
ip netns exec "$r1" sysctl -qw net.ipv4.ip_forward=1 net.ipv4.icmp_echo_ignore_all=1
ip netns exec "$r2" sysctl -qw net.ipv4.ip_forward=1
ip netns exec "$c" sysctl -qw net.ipv4.ping_group_range="0 2147483647"

# A management port that is open on r1's second LAN address.
ip netns exec "$r1" python3 -c '
import socket, time
s = socket.socket(); s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
s.bind(("10.201.1.50", 8291)); s.listen(16)
while True:
    conn, _ = s.accept(); conn.close()
' &
listener_pid=$!
sleep 0.3

cd "$crate_dir"
export NETPROBE_LAB=1 RUST_BACKTRACE=0
ip netns exec "$c" "$binary" --test-threads=1 "$@"
ip netns exec "$c" "$unit_binary" --test-threads=1 lab_ "$@"
