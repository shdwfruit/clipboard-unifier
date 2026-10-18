#!/usr/bin/env bash
# End-to-end: real macOS pasteboard <-> real X11 clipboard in a Linux container.
#
#   cargo build --release && docker build -f scripts/e2e/Dockerfile -t clipbridge-e2e .
#   scripts/e2e/run.sh
#
# Saves the Mac's (text) clipboard first and restores it at the end.
set -euo pipefail
cd "$(dirname "$0")/../.."
BIN=./target/release/clipbridge
WORK=$(mktemp -d /tmp/clipbridge-e2e.XXXXXX)
BACKUP=/tmp/clipbridge-clip-backup.txt
pbpaste > "$BACKUP" || true

cleanup() {
  [[ -n "${MAC_PID:-}" ]] && kill "$MAC_PID" 2>/dev/null || true
  docker rm -f clipbridge-e2e >/dev/null 2>&1 || true
  pbcopy < "$BACKUP"
}
trap cleanup EXIT

"$BIN" --config "$WORK/config.toml" init --name e2e-mac >/dev/null
CODE=$("$BIN" --config "$WORK/config.toml" code)

# Linux peer listens on 47321 inside the container, published on the Mac as 47400.
docker rm -f clipbridge-e2e >/dev/null 2>&1 || true
docker run -d --name clipbridge-e2e --hostname e2e-linux -p 47400:47321 \
  -e CLIPBRIDGE_CODE="$CODE" clipbridge-e2e >/dev/null
lx() { docker exec clipbridge-e2e sh -c "export DISPLAY=:99; $*"; }

"$BIN" --config "$WORK/config.toml" run --no-discovery --port 47322 --peer 127.0.0.1:47400 \
  > "$WORK/mac.log" 2>&1 &
MAC_PID=$!
disown "$MAC_PID"

for _ in $(seq 50); do grep -q "connected to" "$WORK/mac.log" && break; sleep 0.2; done
grep "connected to" "$WORK/mac.log"

fail() { echo "FAIL: $*"; echo "--- mac log"; cat "$WORK/mac.log"; echo "--- linux log"; docker logs clipbridge-e2e; exit 1; }
wait_eq() { # wait_eq <description> <expected> <command...>
  local want=$1; shift
  for _ in $(seq 50); do [[ "$("$@" 2>/dev/null)" == "$want" ]] && return 0; sleep 0.1; done
  return 1
}

echo "== text: Mac -> Linux"
T1="from mac $RANDOM ✓"
printf '%s' "$T1" | pbcopy
wait_eq "$T1" lx xclip -o -selection clipboard || fail "Linux didn't get Mac text"
echo "ok: Linux clipboard = '$(lx xclip -o -selection clipboard)'"

echo "== text: Linux -> Mac"
T2="from linux $RANDOM ✓"
lx "printf '%s' '$T2' | xclip -i -selection clipboard"
wait_eq "$T2" pbpaste || fail "Mac didn't get Linux text"
echo "ok: Mac clipboard = '$(pbpaste)'"

echo "== image: Linux -> Mac"
python3 -I scripts/e2e/make_png.py "$WORK/test.png"
docker cp "$WORK/test.png" clipbridge-e2e:/tmp/test.png
lx xclip -i -selection clipboard -t image/png /tmp/test.png
for _ in $(seq 50); do osascript -e 'clipboard info' | grep -q PNGf && break; sleep 0.1; done
osascript -e 'clipboard info' | grep -q PNGf || fail "Mac didn't get the image"
echo "ok: Mac clipboard info: $(osascript -e 'clipboard info')"

echo "== image: Mac -> Linux (different size, so we can tell it apart)"
python3 -I scripts/e2e/make_png.py "$WORK/test2.png" 40 30
osascript -e "set the clipboard to (read (POSIX file \"$WORK/test2.png\") as «class PNGf»)"
# PNG IHDR: width and height are big-endian u32s at byte offsets 16 and 20.
dims() { lx "xclip -o -selection clipboard -t image/png | head -c 24 | tail -c 8 | od -An -tu1" | tr -s ' ' | sed 's/^ //'; }
wait_eq "0 0 0 40 0 0 0 30" dims || fail "Linux didn't get the 40x30 image (have: $(dims))"
echo "ok: Linux clipboard has a 40x30 image/png ($(lx "xclip -o -selection clipboard -t image/png | wc -c") bytes)"

sleep 2 # let any echo show up
echo "== transfer log (expect exactly one line per transfer, no ping-pong)"
echo "-- mac"; grep -E "local copy|received" "$WORK/mac.log"
echo "-- linux"; docker logs clipbridge-e2e 2>&1 | grep -E "local copy|received"
MAC_SENT=$(grep -c "local copy" "$WORK/mac.log"); MAC_RECV=$(grep -c "received" "$WORK/mac.log")
LX_SENT=$(docker logs clipbridge-e2e 2>&1 | grep -c "local copy"); LX_RECV=$(docker logs clipbridge-e2e 2>&1 | grep -c "received")
[[ $MAC_SENT == 2 && $MAC_RECV == 2 && $LX_SENT == 2 && $LX_RECV == 2 ]] \
  || fail "expected 2 sends and 2 receives on each side, got mac $MAC_SENT/$MAC_RECV linux $LX_SENT/$LX_RECV"
echo "PASS"
