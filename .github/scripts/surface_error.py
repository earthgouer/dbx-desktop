#!/usr/bin/env python3
"""Surface the Build AppImage failure log via the GitHub Contents API.

Runs only on workflow failure. Reads build.log (produced by the Build step
with `tee`), then writes it to `build.log` on the `ci-logs` branch using the
Contents API. The repo's Issues are disabled, so we use a file instead; the
`ci-logs` branch is not `master`, so this does not re-trigger the push-driven
build. The file is publicly readable (public repo) without a token.

Requires GITHUB_TOKEN (provided as github.token) and `contents: write`.
"""
import os
import json
import base64
import urllib.request
import urllib.error

TOKEN = os.environ.get("GH_TOKEN") or os.environ.get("GITHUB_TOKEN")
REPO = os.environ.get("GITHUB_REPOSITORY", "earthgouer/dbx-desktop")
BRANCH = "ci-logs"
PATH = "build.log"
API = f"https://api.github.com/repos/{REPO}/contents/{PATH}"


def req(method, url, data=None):
    headers = {
        "Authorization": f"Bearer {TOKEN}",
        "Accept": "application/vnd.github+json",
        "User-Agent": "ci-surface",
    }
    if data is not None:
        headers["Content-Type"] = "application/json"
        data = data.encode()
    r = urllib.request.Request(url, data=data, method=method, headers=headers)
    try:
        with urllib.request.urlopen(r, timeout=30) as resp:
            return resp.status, json.loads(resp.read().decode())
    except urllib.error.HTTPError as e:
        return e.code, e.read().decode()


try:
    content = open("build.log", "rb").read()[-12000:]
except Exception as e:
    content = f"(build.log unavailable: {e})".encode()

b64 = base64.b64encode(content).decode()

# Fetch existing file SHA on the branch (for update vs create).
status, existing = req("GET", f"{API}?ref={BRANCH}")
sha = existing.get("sha") if status == 200 else None

payload = {"message": "ci: build log (auto)", "content": b64, "branch": BRANCH}
if sha:
    payload["sha"] = sha

status, resp = req("PUT", API, json.dumps(payload))
if status in (200, 201):
    print(f"uploaded build.log to branch {BRANCH} (HTTP {status})")
else:
    print(f"upload FAILED HTTP {status}: {str(resp)[:500]}")
