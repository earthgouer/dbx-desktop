#!/usr/bin/env bash
# Apply mobile-only project patches after `tauri android init` / `tauri ios init`.
#
# The mobile app navigates its webview to a plain http://host:port URL entered
# by the user (no TLS on a LAN), so:
#   - Android needs android:usesCleartextTraffic="true" on <application>
#   - iOS needs an App Transport Security exception (NSAllowsArbitraryLoads)
#
# Idempotent: safe to run repeatedly; each patch is skipped when present.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

patch_android_manifest() {
    local manifest="$ROOT/src-tauri/gen/android/app/src/main/AndroidManifest.xml"
    if [ ! -f "$manifest" ]; then
        echo "skip: AndroidManifest.xml not found ($manifest)" >&2
        return 0
    fi
    if grep -q 'usesCleartextTraffic' "$manifest"; then
        echo "ok: usesCleartextTraffic already set"
        return 0
    fi
    perl -pi -e 's/<application\b/<application\n        android:usesCleartextTraffic="true"/' "$manifest"
    grep -q 'usesCleartextTraffic' "$manifest" \
        || { echo "::error::failed to patch $manifest" >&2; exit 1; }
    echo "ok: added usesCleartextTraffic to AndroidManifest.xml"
}

patch_ios_plist() {
    local plist
    plist="$(find "$ROOT/src-tauri/gen/apple" -name Info.plist -path '*_iOS*' 2>/dev/null | head -n 1 || true)"
    if [ -z "$plist" ]; then
        echo "skip: iOS Info.plist not found under src-tauri/gen/apple" >&2
        return 0
    fi
    if /usr/libexec/PlistBuddy -c 'Print :NSAppTransportSecurity' "$plist" >/dev/null 2>&1; then
        if /usr/libexec/PlistBuddy -c 'Print :NSAppTransportSecurity:NSAllowsArbitraryLoads' "$plist" 2>/dev/null | grep -qi true; then
            echo "ok: ATS exception already present"
            return 0
        fi
        /usr/libexec/PlistBuddy -c 'Set :NSAppTransportSecurity:NSAllowsArbitraryLoads true' "$plist"
    else
        /usr/libexec/PlistBuddy -c 'Add :NSAppTransportSecurity dict' "$plist"
        /usr/libexec/PlistBuddy -c 'Add :NSAppTransportSecurity:NSAllowsArbitraryLoads bool true' "$plist"
    fi
    /usr/libexec/PlistBuddy -c 'Print :NSAppTransportSecurity' "$plist" >/dev/null \
        || { echo "::error::failed to patch $plist" >&2; exit 1; }
    echo "ok: added ATS exception to $(basename "$(dirname "$plist")")/Info.plist"
}

case "${1:-all}" in
    android) patch_android_manifest ;;
    ios)     patch_ios_plist ;;
    all)     patch_android_manifest; patch_ios_plist ;;
    *)
        echo "usage: $0 [android|ios|all]" >&2
        exit 2
        ;;
esac
