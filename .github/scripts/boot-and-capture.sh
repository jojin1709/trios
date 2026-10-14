#!/usr/bin/env bash
# Boots the TriOS UEFI disk image headless in QEMU (OVMF firmware) and captures:
#   * the COM1 serial console (kernel UART at 0x3F8) -> <out>/serial.log
#   * timed screenshots of the VGA framebuffer via the QEMU monitor's
#     `screendump` command (PPM), converted to PNG with ImageMagick
#
# Usage: boot-and-capture.sh <disk-image> <output-dir>
# Env:   SHOT_TIMES   space-separated seconds-after-boot (default: "3 8 15 30")
#
# Notes:
#   * GitHub-hosted runners have no /dev/kvm, so QEMU runs under TCG and the
#     guest is slow; the default timings still catch the UEFI/splash phase
#     (~3 s) and the desktop once it is up (~15-30 s).
#   * `-display none` still supports `screendump` because `-vga std` keeps
#     the framebuffer alive.
set -euo pipefail

IMAGE="${1:?usage: boot-and-capture.sh <disk-image> <output-dir>}"
OUT="${2:-boot-artifacts}"

if [ ! -f "$IMAGE" ]; then
  echo "::error::disk image not found: $IMAGE (run 'cargo build --release' first)"
  exit 1
fi
IMAGE="$(readlink -f "$IMAGE")"
mkdir -p "$OUT"
OUT="$(readlink -f "$OUT")"

# --- locate OVMF firmware (Ubuntu 24.04 vs older layouts) --------------------
# 24.04 ships 4M pflash CODE+VARS pairs; older images ship the 2M pair or a
# single combined OVMF.fd (then we fall back to -bios).
CODE=""
VARS_TEMPLATE=""
if [ -f /usr/share/OVMF/OVMF_CODE_4M.fd ] && [ -f /usr/share/OVMF/OVMF_VARS_4M.fd ]; then
  CODE=/usr/share/OVMF/OVMF_CODE_4M.fd
  VARS_TEMPLATE=/usr/share/OVMF/OVMF_VARS_4M.fd
elif [ -f /usr/share/OVMF/OVMF_CODE.fd ] && [ -f /usr/share/OVMF/OVMF_VARS.fd ]; then
  CODE=/usr/share/OVMF/OVMF_CODE.fd
  VARS_TEMPLATE=/usr/share/OVMF/OVMF_VARS.fd
elif [ -f /usr/share/ovmf/OVMF.fd ]; then
  CODE=/usr/share/ovmf/OVMF.fd
elif [ -f /usr/share/OVMF/OVMF.fd ]; then
  CODE=/usr/share/OVMF/OVMF.fd
else
  echo "::error::no OVMF firmware found under /usr/share/OVMF or /usr/share/ovmf"
  exit 1
fi
echo "image     : $IMAGE"
echo "output dir: $OUT"
echo "OVMF code : $CODE"
echo "OVMF vars : ${VARS_TEMPLATE:-(combined image -> -bios fallback)}"

SERIAL="$OUT/serial.log"
MONITOR="$OUT/monitor.sock"
rm -f "$SERIAL" "$MONITOR" "$OUT"/shot-*.ppm

QEMU_ARGS=(
  -machine q35
  -m 256M
  -cpu max
  -vga std
  -display none
  -serial "file:$SERIAL"
  -monitor "unix:$MONITOR,server,nowait"
  -no-reboot
  -nic none
)
if [ -n "$VARS_TEMPLATE" ]; then
  # OVMF writes NVRAM back to the VARS store; it must be a writable copy.
  VARS_COPY="$OUT/OVMF_VARS.copy.fd"
  cp "$VARS_TEMPLATE" "$VARS_COPY"
  QEMU_ARGS+=(
    -drive "if=pflash,format=raw,readonly=on,file=$CODE"
    -drive "if=pflash,format=raw,file=$VARS_COPY"
  )
else
  QEMU_ARGS+=(-bios "$CODE")
fi
QEMU_ARGS+=(-drive "format=raw,file=$IMAGE")

echo "launching: qemu-system-x86_64 ${QEMU_ARGS[*]}"
qemu-system-x86_64 "${QEMU_ARGS[@]}" &
QEMU_PID=$!

cleanup() {
  if kill -0 "$QEMU_PID" 2>/dev/null; then
    kill "$QEMU_PID" 2>/dev/null || true
    wait "$QEMU_PID" 2>/dev/null || true
  fi
}
trap cleanup EXIT INT TERM

# --- wait for the QEMU monitor socket ----------------------------------------
ready=0
for _ in $(seq 1 100); do
  if [ -S "$MONITOR" ]; then
    ready=1
    break
  fi
  if ! kill -0 "$QEMU_PID" 2>/dev/null; then
    break
  fi
  sleep 0.2
done
if [ "$ready" -ne 1 ]; then
  echo "::error::QEMU exited before its monitor socket appeared (bad args / firmware?)"
  exit 1
fi
echo "QEMU up (pid $QEMU_PID), capturing screenshots ..."

# --- monitor helpers ---------------------------------------------------------
# Send one HMP command over the unix monitor socket; -T is an inactivity
# timeout so socat exits after the (qemu) prompt goes quiet.
monitor_cmd() {
  printf '%s\n' "$1" | socat -T 2 - "UNIX-CONNECT:$MONITOR" >/dev/null 2>&1 || true
}

shoot() {
  local t="$1"
  local ppm="$OUT/shot-${t}s.ppm"
  local png="$OUT/shot-${t}s.png"
  monitor_cmd "screendump $ppm"
  # wait for QEMU to finish writing the PPM (it is ~2 MB at 1024x768)
  for _ in $(seq 1 25); do
    [ -s "$ppm" ] && break
    sleep 0.2
  done
  if [ -s "$ppm" ]; then
    if convert "$ppm" "$png"; then
      echo "captured $png ($(du -h "$png" | cut -f1))"
    else
      echo "::warning::failed to convert $ppm -> png"
    fi
  else
    echo "::warning::screendump produced no $ppm (QEMU dead? monitor busy?)"
  fi
}

# --- timed screenshots -------------------------------------------------------
SHOT_TIMES=${SHOT_TIMES:-"3 8 15 30"}
prev=0
for t in $SHOT_TIMES; do
  sleep $((t - prev))
  shoot "$t"
  prev=$t
done

# Let the last screendump settle, then kill QEMU (trap also covers timeouts).
sleep 1
cleanup
trap - EXIT INT TERM

# --- report ------------------------------------------------------------------
echo
if [ -f "$SERIAL" ]; then
  echo "--- serial.log: $(wc -c < "$SERIAL") bytes, last 40 lines ---"
  tail -n 40 "$SERIAL" || true
  echo "--- end of serial.log ---"
else
  echo "::warning::serial.log was never created (guest never reached COM1, or UEFI did not boot the image)"
fi

shots=$(find "$OUT" -maxdepth 1 -name 'shot-*.png' | wc -l)
echo
echo "done: $shots screenshot(s) in $OUT"
