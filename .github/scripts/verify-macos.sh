#!/usr/bin/env bash
# Verify the installed DSH Desktop app on macOS:
#   launch the app, confirm it keeps running, confirm the dsh web UI is
#   reachable, capture a screenshot, and emit a PASS/FAIL verdict.
set -u

PORT=3080
TIMEOUT=150

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

# give the UI a moment to paint, then bring the window to the front
sleep 8
open "$APP_PATH" 2>/dev/null || true
sleep 3

# --- screenshot ---------------------------------------------------------------
shot_ok=0
if screencapture -x screenshot-macos.png 2>/dev/null && [ -s screenshot-macos.png ]; then
  shot_ok=1
  echo "Screenshot saved: screenshot-macos.png"
else
  echo "screencapture failed (screen recording may be unavailable on this runner)."
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

{
  echo "## DSH Desktop verification report (macOS)"
  echo ""
  echo "- Installed app: $APP_PATH"
  echo "- App process running: $proc_alive"
  echo "- dsh web UI reachable (port $PORT): $server_ok"
  echo "- UI loaded (TCP connection to port $PORT): $ui_connected"
  echo "- Screenshot captured: $shot_ok"
  echo "- Verdict: **$verdict**"
  echo "- Detail: $detail"
  echo ""
  echo "### App log (tail)"
  echo '```'
  tail -n 40 app.log 2>/dev/null || true
  echo '```'
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
