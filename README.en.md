# DSH Desktop Lite

**English** | [中文](README.md)

DSH Desktop is a desktop launcher for [DSH (DeepSeek Harness)](https://github.com/deepseek-ai/deepseek-harness). Powered by Tauri 2, it automatically detects and starts the local `dsh web` service and opens its web interface inside the app window.


## Features

- **Lightning fast**: Built natively on Tauri 2 — small installer, low memory footprint.
- **Lightweight entry**: This project is only a launcher/entry point for DSH and does not bundle DSH itself. Install the DSH tool first:

  ```bash
  npm install -g @deepseek-ai/dsh
  ```

- **Auto detection**: On startup, checks whether `dsh web` is already running, on the default port (3080) or any other port that responds with the dsh web page signature.
- **Auto start**: If no running instance is found, it locates the `dsh` command and launches `dsh web --port 3080` automatically — no manual commands needed.
- **In-app browsing**: Once the service is ready, the dsh web interface opens directly inside the app window.
- **Cross-platform**: Supports Windows, macOS and Linux, correctly locating the `dsh` command on every platform.
- **Mobile**: Ships as an Android APK and an iOS app. On mobile, no local service is started — enter the IP address and port of a computer on the same network and connect directly to the running `dsh web` instance (the last address is remembered and auto-reconnected).
- **Clean shutdown**: On exit, only the dsh process started by this app is terminated — user-started instances are never killed.

## Download

Download the installer for your platform from the [GitHub Releases](https://github.com/smanx/dsh-desktop/releases) page.

## Screenshots

<p align="center"><img src="doc/image-en.jpg" alt="DSH Desktop main window" width="600"></p>

## Requirements

- [Node.js](https://nodejs.org/) (20 or later recommended)
- [Rust](https://www.rust-lang.org/) stable
- [Tauri 2 platform prerequisites](https://tauri.app/start/prerequisites/) (compilation dependencies per OS, e.g. WebView2 on Windows, webkit2gtk on Linux)
- DSH tool installed globally (see install command above)

## Development

```bash
# Install dependencies
npm install

# Start development mode (hot reload)
npm run tauri dev
```

## Build & Package

```bash
# Build the installer for the current platform
npm run tauri build
```

Package targets for each platform are configured in `src-tauri/tauri.conf.json`; NSIS (Windows) is used by default.

### Automated Releases (GitHub Actions)

The repository ships a [build-release.yml](.github/workflows/build-release.yml) workflow:

- **Push a `v*` tag**: builds the Windows/macOS/Linux installers and the Android APK, then publishes an official Release.
- **Manual dispatch**: builds the same artifacts but produces only a **standalone draft** (`manual-build`, titled with the run number). It is fully isolated from every existing tag and official release — nothing gets bound to, overwritten or modified. Review it and handle the draft as you see fit.

### Mobile Builds (GitHub Actions)

The same workflow also produces mobile packages in CI — no local Android/iOS toolchain required:

- **Android**: the `build-android` job builds a signed arm64 **release APK** (aligned and signed with zipalign + apksigner — installable and updatable in place) and attaches it to the GitHub Release as `*.apk`. All signing material is injected via GitHub Secrets — configure these under **Settings → Secrets and variables → Actions**:

  | Secret | Description |
  |---|---|
  | `ANDROID_KEYSTORE_BASE64` | the keystore file encoded as base64 |
  | `ANDROID_KEYSTORE_PASSWORD` | keystore password |
  | `ANDROID_KEY_ALIAS` | key alias (optional, defaults to `dsh-desktop`) |

  Generate a keystore locally and get its base64 (Windows PowerShell):

  ```powershell
  & "$env:JAVA_HOME\bin\keytool.exe" -genkeypair -v -keystore dsh-release.keystore `
    -storetype PKCS12 -alias dsh-desktop -keyalg RSA -keysize 2048 -validity 10000 `
    -storepass "your-password" -dname "CN=dsh-desktop, C=CN"
  [Convert]::ToBase64String([IO.File]::ReadAllBytes("dsh-release.keystore")) # paste into the Secret
  ```

  On macOS/Linux use `base64 -i dsh-release.keystore`. **Always back up the keystore file**: if it is lost, updates can no longer be signed with the same identity.
- **iOS**: the `build-ios` job builds an arm64 device **IPA** (archived with `--no-sign`, so it is unsigned — install on devices via sideloading tools like Sideloadly/AltStore, or wire Apple developer certificate secrets to switch to signed export). It is attached to the GitHub Release/draft as `*.ipa` alongside the APK.
- **Icons**: both jobs run `tauri icon app-icon.png` after project init, so the Android launcher icons, the iOS AppIcon and the desktop icons are all generated from the same source image for a consistent look.

Mobile usage: connect the phone to the same network as the computer, run `dsh web --port 3080` there, then enter the computer's LAN IP (e.g. `192.168.1.100`) and port in the app. To allow plain HTTP, mobile builds enable Android cleartext traffic (including release builds, patched automatically by [.github/scripts/patch-mobile.sh](.github/scripts/patch-mobile.sh)) and an iOS ATS exception.

## How It Works

1. On startup, the app calls the `check_dsh` command to check the service status.
2. If dsh is already running (default port 3080, or any listening port responding with the dsh web page signature), it navigates directly.
3. Otherwise it locates the `dsh` command (PATH first, then common install locations), launches `dsh web --port 3080` and polls until it is ready.
4. Once ready, the web interface opens inside the window; on exit, if dsh was started by this app, its process tree is terminated as well.

**Mobile**: no local process is detected or started. The app shows a connect form (IP address + port), calls `connect_remote` to probe whether the target serves dsh, then opens the remote interface in-app. The last address is saved locally and reconnected automatically on the next launch.

## Project Structure

```
dsh-desktop
├── src/                    # Frontend (vanilla HTML/CSS/JS)
│   ├── index.html
│   ├── main.js             # Calls Tauri commands and handles navigation
│   └── styles.css
├── src-tauri/              # Tauri backend (Rust)
│   ├── src/
│   │   ├── main.rs
│   │   └── lib.rs          # Core logic: detection, startup, port probing
│   ├── capabilities/
│   └── tauri.conf.json     # App configuration
├── doc/                    # Docs assets (screenshots, etc.)
└── .github/                # CI build & release workflows + mobile project patch script
```

## Logs

When the app starts dsh, its output is written to `dsh-web.log` under the system app-log directory. If startup fails or times out, check this log to troubleshoot.

## License

This project is open source under the [MIT License](LICENSE).

```
MIT License

Copyright (c) 2026 dsh-desktop contributors

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```
