#!/usr/bin/env bash
# Run only inside an isolated Ubuntu 24.04 acceptance container.
set -euo pipefail
[[ -f /.dockerenv && $# == 1 ]] || { echo "Pass output path inside an isolated Docker container." >&2; exit 2; }
output="$1"
[[ "$output" == /tmp/* ]] || { echo "Acceptance output must be below /tmp." >&2; exit 2; }
mkdir -p "$output"
export SSHCC_DATA_DIR="$output/state" SSHCC_SOCKET="$output/control.sock"
export WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS=1
export WEBKIT_DISABLE_DMABUF_RENDERER=1 LIBGL_ALWAYS_SOFTWARE=1 GDK_BACKEND=x11
exe=/src/target/release/sshcc
tray=/src/target/release/sshcc-desktop
"$exe" init --role both --owner "$(id -u)" --label benchmark:ubuntu24-xvfb > "$output/init.json"
"$exe" service run > "$output/service.log" 2>&1 &
service_pid=$!
tint2 > "$output/tint2.log" 2>&1 &
panel_pid=$!
tray_pid=""
cleanup() {
  [[ -z "$tray_pid" ]] || kill "$tray_pid" 2>/dev/null || true
  kill "$service_pid" "$panel_pid" 2>/dev/null || true
}
trap cleanup EXIT
for attempt in $(seq 1 50); do
  "$exe" request system.status > "$output/service-status.json" 2>/dev/null && break
  sleep 0.2
done
python3 /src/scripts/seed-benchmark.py --sshcc "$exe" --isolated-state "$SSHCC_DATA_DIR"
"$tray" --benchmark-cycles 10 --benchmark-report "$output/windows.json" > "$output/tray.log" 2>&1 &
tray_pid=$!
(
  sleep 3
  xwininfo -root -tree > "$output/x11-window-tree.txt" 2>&1 || true
  ps -eo pid,ppid,rss,comm,args > "$output/processes-during-window.txt"
) &
python3 /src/scripts/benchmark.py --sshcc "$exe" --service-pid "$service_pid" --tray-pid "$tray_pid" \
  --window-report "$output/windows.json" --output "$output/resources.json" --seconds 600 --interval 1