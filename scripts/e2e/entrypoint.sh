#!/bin/sh
# Start a headless X server, then clipbridge against it.
set -e
Xvfb :99 -screen 0 1024x768x24 >/dev/null 2>&1 &
export DISPLAY=:99
sleep 1
exec clipbridge run --no-discovery "$@"
