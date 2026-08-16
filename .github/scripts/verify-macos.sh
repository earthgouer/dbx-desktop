#!/usr/bin/env bash
# Verify the installed DSH Desktop app on macOS:
#   launch the app, confirm it keeps running, confirm the dsh web UI is
#   reachable, capture a screenshot, and emit a PASS/FAIL verdict.
set -u

PORT=3080
TIMEOUT=240

APP_PATH="/Applications/dsh-desktop.app"
BIN="$APP_PATH/Contents/MacOS/dsh-desktop"

if [ ! -x "$BIN" ]; then
  echo "::error::Installed app binary not found at $BIN."
  exit 1
fi

echo "Launching installed app: $BIN"
# Launch the binary directly so it inherits the runner's PATH (the dsh CLI and
# node are installed under the hosted tool cache).
"$BIN" >app.log 2>&1 &
APP_PID=$!
echo "App launched, PID $APP_PID"

# (e) screenshot timeline: at launch (t=0), then at 5s and 10s
take_screenshot() {
  local out="$1"
  open "$APP_PATH" 2>/dev/null || true
  sleep 1
  if screencapture -x "$out" 2>/dev/null && [ -s "$out" ]; then
    echo "Screenshot saved: $out"
    return 0
  fi
  echo "Screenshot failed: $out"
  return 1
}
shot_ok=0
take_screenshot screenshot-macos-1.png && shot_ok=1
sleep 5
take_screenshot screenshot-macos-2.png && shot_ok=1
sleep 5
take_screenshot screenshot-macos-3.png && shot_ok=1

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
if lsof -nP -iTCP:$PORT -sTCP:ESTABLISHED >/dev/null 2>&1; then
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
for c in "$HOME/Library/Logs/com.dsh.desktop/dsh-web.log" \
         "$HOME/Library/Application Support/com.dsh.desktop/logs/dsh-web.log"; do
  [ -f "$c" ] && DSH_LOG="$c" && break
done
if [ -n "$DSH_LOG" ]; then
  echo "dsh-web.log: $DSH_LOG"
else
  echo "dsh-web.log: not found"
fi

{
  echo "## DSH Desktop verification report (macOS)"
  echo ""
  echo "- Installed app: $APP_PATH"
  echo "- App process running: $proc_alive"
  echo "- dsh web UI reachable (port $PORT): $server_ok"
  echo "- UI loaded (TCP connection to port $PORT): $ui_connected"
  echo "- Screenshots: screenshot-macos-1.png (t=0s), -2.png (t=5s), -3.png (t=10s)"
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
  echo "::error::DSH Desktop verification FAILED on macOS."
  exit 1
fi
echo "::notice::DSH Desktop verification PASSED on macOS."
