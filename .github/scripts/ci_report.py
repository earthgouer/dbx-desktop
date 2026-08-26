#!/usr/bin/env python3
"""Report the AppImage build result to the public `ci-logs` branch.

Runs on both success and failure (BUILD_STATUS=ok|fail). Writes a small,
human-readable file to the `ci-logs` branch via the GitHub Contents API so
the result can be inspected without a token (the repo is public):

  - on failure: ci-logs/build.log  -> last ~150 lines of the build output
  - on success: ci-logs/success.log -> AppImage name, size, sha256

The full build.log is also uploaded as a workflow artifact for deeper
inspection. Requires contents: write (provided by github.token).
"""
import os
import json
import base64
import glob
import hashlib

import urllib.request
import urllib.error

TOKEN = os.environ.get("GH_TOKEN") or os.environ.get("GITHUB_TOKEN")
REPO = os.environ.get("GITHUB_REPOSITORY", "earthgouer/dbx-desktop")
BRANCH = "ci-logs"
BASE = f"https://api.github.com/repos/{REPO}"
STATUS = os.environ.get("BUILD_STATUS", "fail")


def api(method, url, data=None):
    headers = {
        "Authorization": f"Bearer {TOKEN}",
        "Accept": "application/vnd.github+json",
        "User-Agent": "ci-report",
        "X-GitHub-Api-Version": "2022-11-28",
    }
    body = json.dumps(data).encode() if data is not None else None
    if body is not None:
        headers["Content-Type"] = "application/json"
    req = urllib.request.Request(url, data=body, method=method, headers=headers)
    try:
        with urllib.request.urlopen(req, timeout=30) as r:
            return r.status, r.read().decode()
    except urllib.error.HTTPError as e:
        return e.code, e.read().decode()
    except Exception as e:  # noqa: BLE001
        return 0, str(e)


def ensure_branch():
    st, _ = api("GET", f"{BASE}/branches/{BRANCH}")
    if st == 200:
        return
    st, resp = api("GET", f"{BASE}/git/refs/heads/master")
    sha = None
    try:
        sha = json.loads(resp).get("object", {}).get("sha")
    except Exception:  # noqa: BLE001
        pass
    if sha:
        api("POST", f"{BASE}/git/refs", {"ref": f"refs/heads/{BRANCH}", "sha": sha})


def put_file(path, content_bytes, message):
    ensure_branch()
    url = f"{BASE}/contents/{path}"
    st, existing = api("GET", f"{url}?ref={BRANCH}")
    sha = None
    try:
        sha = json.loads(existing).get("sha")
    except Exception:  # noqa: BLE001
        pass
    b64 = base64.b64encode(content_bytes).decode()
    payload = {"message": message, "content": b64, "branch": BRANCH}
    if sha:
        payload["sha"] = sha
    st, resp = api("PUT", url, payload)
    if st in (200, 201):
        print(f"OK: wrote {path} to {BRANCH} (HTTP {st})")
    else:
        print(f"FAIL: write {path} HTTP {st}: {resp[:500]}")


if STATUS == "ok":
    bundle = "src-tauri/target/x86_64-unknown-linux-gnu/release/bundle/appimage"
    files = sorted(glob.glob(os.path.join(bundle, "*.AppImage")))
    lines = ["BUILD_STATUS=ok"]
    if files:
        f = files[0]
        size = os.path.getsize(f)
        h = hashlib.sha256()
        with open(f, "rb") as fh:
            for chunk in iter(lambda: fh.read(1 << 16), b""):
                h.update(chunk)
        lines.append(f"appimage={os.path.basename(f)}")
        lines.append(f"size_bytes={size}")
        lines.append(f"sha256={h.hexdigest()}")
    else:
        lines.append("appimage=NONE_FOUND")
    put_file("success.log", ("\n".join(lines) + "\n").encode(), "ci: build success")
else:
    try:
        with open("build.log", "r", errors="replace") as f:
            raw = f.read()
        # Push enough context: first 30 lines (headers/versions) + last 300.
        lines = raw.splitlines()
        head = lines[:30]
        tail = lines[-300:]
        body = ["=== build.log head (first %d lines) ===" % len(head)] + head
        body += ["", "=== build.log tail (last %d of %d lines) ===" % (len(tail), len(lines))] + tail
        content = "\n".join(body)
        if len(content) > 60000:
            content = content[:60000] + "\n...[truncated]..."
    except Exception as e:  # noqa: BLE001
        content = f"(build.log unavailable: {e})"
    put_file("build.log", content.encode(), "ci: build log (auto)")
