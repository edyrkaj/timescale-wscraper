#!/bin/sh
set -eu

# Cloudflare managed challenges detect headless Chrome; run headed under Xvfb.
if [ "${USE_XVFB:-1}" = "1" ]; then
  export DISPLAY="${DISPLAY:-:99}"
  if ! pgrep -x Xvfb >/dev/null 2>&1; then
    Xvfb "$DISPLAY" -screen 0 1365x900x24 -ac +extension RANDR >/tmp/xvfb.log 2>&1 &
    # Wait until the display is ready
    for _ in 1 2 3 4 5 6 7 8 9 10; do
      if xdpyinfo -display "$DISPLAY" >/dev/null 2>&1; then
        break
      fi
      sleep 0.3
    done
  fi
fi

exec /app/rust_scraper_timescale
