#!/usr/bin/env bash
# Verify the installed DSH Desktop app on Linux (headless, under Xvfb):
#   launch the app, confirm it keeps running, confirm the dsh web UI is
#   reachable, capture a screenshot, and emit a PASS/FAIL verdict.
set -u

PORT=3080
TIMEOUT=240
DISPLAY=:99

# --- start a virtual display --------------------------------------------------
Xvfb "$DISPLAY" -screen 0 1440x1000x24 >/tmp/xvfb.log 2>&1 &
XVFB_PID=$!
cleanup() {
  kill "$XVFB_PID" 2>/dev/null || true
  pkill -x dsh-desktop 2>/dev/null || true
}
trap cleanup EXIT
sleep 2
export DISPLAY

# --- locate the installed binary ----------------------------------------------
BIN=""
for c in /usr/bin/dsh-desktop /usr/local/bin/dsh-desktop /usr/lib/dsh-desktop/dsh-desktop; do
  if [ -x "$c" ]; then BIN="$c"; break; fi
done
if [ -z "$BIN" ]; then
  BIN="$(command -v dsh-desktop 2>/dev/null || true)"
fi
if [ -z "$BIN" ] || [ ! -x "$BIN" ]; then
  echo "::error::Installed dsh-desktop binary not found."
  exit 1
fi
echo "Launching installed app: $BIN"

# WebKitGTK on headless CI: force software rendering to avoid EGL / dma-buf
# crashes, so the window actually paints and can be screenshotted.
export WEBKIT_DISABLE_DMABUF_RENDERER=1
export WEBKIT_DISABLE_COMPOSITING_MODE=1
export GDK_BACKEND=x11

"$BIN" >app.log 2>&1 &
APP_PID=$!
echo "App launched, PID $APP_PID"

# (e) screenshot timeline: at launch (t=0), then at 5s and 10s
take_screenshot() {
  local out="$1"
  if command -v import >/dev/null 2>&1; then
    if import -display "$DISPLAY" -window root "$out" >/dev/null 2>&1 && [ -s "$out" ]; then
      echo "Screenshot saved: $out"
      return 0
    fi
    echo "Screenshot failed: $out"
  else
    echo "ImageMagick 'import' not available."
  fi
  return 1
}
shot_ok=0
take_screenshot screenshot-linux-1.png && shot_ok=1
sleep 5
take_screenshot screenshot-linux-2.png && shot_ok=1
sleep 5
take_screenshot screenshot-linux-3.png && shot_ok=1

# --- poll for the dsh web UI --------------------------------------------------
server_ok=0
end=$((SECONDS + TIMEOUT))
while [ "$SECONDS" -lt "$end" ]; do
  if ! kill -0 "$APP_PID" 2>/dev/null; then
    echo "App process exited early (see app.log)."
    break
  fi
  body="$(curl -s --max-time 5 "http://127.0.0.1:$PORT/" || true)"
  if [ -n "$body" ]; then
    low="$(printf '%s' "$body" | tr '[:upper:]' '[:lower:]')"
    if printf '%s' "$low" | grep -qE 'deepseek|harness'; then
      server_ok=1
      echo "dsh web UI is responding on port $PORT."
      break
    fi
  fi
  sleep 3
done

proc_alive=0
if kill -0 "$APP_PID" 2>/dev/null; then
  proc_alive=1
fi

ui_connected=0
if ss -tn 2>/dev/null | awk 'NR>1{print $NF}' | grep -q "127.0.0.1:$PORT"; then
  ui_connected=1
fi

# --- verdict ------------------------------------------------------------------
if [ "$proc_alive" = "1" ] && [ "$server_ok" = "1" ]; then
  verdict=PASS
  detail="App stayed running and the dsh web UI responded on port $PORT."
elif [ "$server_ok" = "1" ]; then
  verdict=FAIL
  detail="dsh web UI responded on port $PORT, but the app process exited."
elif [ "$proc_alive" = "1" ]; then
  verdict=FAIL
  detail="App process is alive but the dsh web UI did not respond on port $PORT."
else
  verdict=FAIL
  detail="App process exited and the dsh web UI did not respond on port $PORT."
fi

# --- collect the app's dsh-web.log (the spawned dsh writes here) --------------
DSH_LOG=""
for c in "$HOME/.local/share/com.dsh.desktop/logs/dsh-web.log" \
         "${XDG_DATA_HOME:-$HOME/.local/share}/com.dsh.desktop/logs/dsh-web.log"; do
  [ -f "$c" ] && DSH_LOG="$c" && break
done
if [ -n "$DSH_LOG" ]; then
  echo "dsh-web.log: $DSH_LOG"
else
  echo "dsh-web.log: not found"
fi

{
  echo "## DSH Desktop verification report (Linux)"
  echo ""
  echo "- Installed app: $BIN"
  echo "- App process running: $proc_alive"
  echo "- dsh web UI reachable (port $PORT): $server_ok"
  echo "- UI loaded (TCP connection to port $PORT): $ui_connected"
  echo "- Screenshots: screenshot-linux-1.png (t=0s), -2.png (t=5s), -3.png (t=10s)"
  echo "- Screenshot captured: $shot_ok"
  echo "- Verdict: **$verdict**"
  echo "- Detail: $detail"
  echo ""
  echo "### App log (tail)"
  echo '```'
  tail -n 40 app.log 2>/dev/null || true
  echo '```'
  if [ -n "$DSH_LOG" ]; then
    echo ""
    echo "### dsh-web.log (tail)"
    echo '```'
    tail -n 40 "$DSH_LOG" 2>/dev/null || true
    echo '```'
  fi
} | tee report.txt

if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
  cat report.txt >> "$GITHUB_STEP_SUMMARY"
fi
if [ -n "${GITHUB_OUTPUT:-}" ]; then
  echo "verdict=$verdict" >> "$GITHUB_OUTPUT"
fi

if [ "$verdict" != "PASS" ]; then
  echo "::error::DSH Desktop verification FAILED on Linux."
  exit 1
fi
echo "::notice::DSH Desktop verification PASSED on Linux."
