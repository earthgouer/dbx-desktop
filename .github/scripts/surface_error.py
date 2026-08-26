#!/usr/bin/env python3
"""Surface the Build AppImage failure log to a GitHub Issue.

Runs only on workflow failure. Reads build.log (produced by the Build step
with `tee`), then creates or updates a single tracking issue so the error is
readable by tooling that cannot access the Actions log API directly.

Requires GITHUB_TOKEN (provided automatically as github.token) and the
`issues: write` workflow permission.
"""
import os
import json
import urllib.request
import urllib.error

TOKEN = os.environ.get("GH_TOKEN") or os.environ.get("GITHUB_TOKEN")
REPO = os.environ.get("GITHUB_REPOSITORY", "earthgouer/dbx-desktop")
API = f"https://api.github.com/repos/{REPO}"
LABEL = "ci-auto-error"
TITLE = "CI Build Error (auto)"


def api(method, path, data=None):
    headers = {
        "Authorization": f"Bearer {TOKEN}",
        "Accept": "application/vnd.github+json",
        "User-Agent": "ci-surface",
        "Content-Type": "application/json",
    }
    req = urllib.request.Request(API + path, data=data, method=method, headers=headers)
    try:
        with urllib.request.urlopen(req, timeout=30) as r:
            return r.status, json.loads(r.read().decode())
    except urllib.error.HTTPError as e:
        return e.code, e.read().decode()


# Read the captured build log (last 12 KB is enough to see the failure).
try:
    log = open("build.log", "rb").read()[-12000:].decode("utf-8", "replace")
except Exception as e:
    log = f"(build.log unavailable: {e})"

body = "## Build AppImage failed (auto-posted by CI)\n\n```\n" + log + "\n```\n"

# Reuse a single tracking issue instead of spawning a new one each run.
status, issues = api("GET", "/issues?state=open&per_page=50")
existing = None
if status == 200:
    for i in issues:
        if any(l.get("name") == LABEL for l in i.get("labels", [])):
            existing = i
            break

payload = json.dumps({"title": TITLE, "body": body, "labels": [LABEL]}).encode()
if existing:
    status, resp = api("PATCH", f"/issues/{existing['number']}", payload)
    print(f"updated issue #{existing['number']} (HTTP {status})")
else:
    status, resp = api("POST", "/issues", payload)
    num = resp.get("number") if status == 201 else "FAILED"
    print(f"created issue #{num} (HTTP {status})")
