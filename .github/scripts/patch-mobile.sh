#!/usr/bin/env bash
# Apply mobile-only project patches after `tauri android init` / `tauri ios init`.
#
# The mobile app navigates its webview to a plain http://host:port URL entered
# by the user (no TLS on a LAN), so:
#   - Android needs android:usesCleartextTraffic="true" (the current Tauri
#     template wires it through a manifestPlaceholder that is only enabled for
#     debug builds, so we force it for release too)
#   - iOS needs an App Transport Security exception (NSAllowsArbitraryLoads)
#
# Idempotent: safe to run repeatedly; each patch is skipped when present.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

patch_android_manifest() {
    local gradle_app="$ROOT/src-tauri/gen/android/app/build.gradle.kts"
    local manifest="$ROOT/src-tauri/gen/android/app/src/main/AndroidManifest.xml"

    # 1) Current Tauri templates drive usesCleartextTraffic via a
    #    manifestPlaceholder that defaults to false and is flipped to true
    #    only for debug builds. Force it for release as well.
    if [ -f "$gradle_app" ] && grep -q 'getByName("release")' "$gradle_app"; then
        if ! perl -0777 -ne 'exit(/getByName\("release"\)\s*\{[^}]*?manifestPlaceholders\["usesCleartextTraffic"\] = "true"/s ? 0 : 1)' "$gradle_app"; then
            perl -0pi -e 's/getByName\("release"\)\s*\{\n/getByName("release") {\n            manifestPlaceholders["usesCleartextTraffic"] = "true"\n/' "$gradle_app"
            grep -q 'getByName("release")' "$gradle_app" || { echo "::error::failed to patch $gradle_app" >&2; exit 1; }
            echo "ok: enabled usesCleartextTraffic for release builds"
        else
            echo "ok: usesCleartextTraffic already enabled for release builds"
        fi
    fi

    # 2) Fallback for older templates without the placeholder: set the
    #    attribute literally on <application>.
    if [ -f "$manifest" ] && ! grep -q 'android:usesCleartextTraffic=' "$manifest"; then
        perl -pi -e 's/<application\b/<application\n        android:usesCleartextTraffic="true"/' "$manifest"
        grep -q 'android:usesCleartextTraffic=' "$manifest" \
            || { echo "::error::failed to patch $manifest" >&2; exit 1; }
        echo "ok: added usesCleartextTraffic to AndroidManifest.xml"
    elif [ -f "$manifest" ]; then
        echo "ok: manifest already handles usesCleartextTraffic"
    fi
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
