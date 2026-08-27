use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::time::Duration;

#[cfg(not(any(target_os = "android", target_os = "ios")))]
use std::fs::OpenOptions;
#[cfg(not(any(target_os = "android", target_os = "ios")))]
use std::path::PathBuf;
#[cfg(not(any(target_os = "android", target_os = "ios")))]
use std::process::{Command, Stdio};
#[cfg(not(any(target_os = "android", target_os = "ios")))]
use std::sync::Mutex;

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

#[cfg(unix)]
use std::os::unix::process::CommandExt;

use serde::Serialize;
#[cfg(not(any(target_os = "android", target_os = "ios")))]
use sysinfo::{ProcessRefreshKind, System, UpdateKind};
#[cfg(not(any(target_os = "android", target_os = "ios")))]
use tauri::{AppHandle, Manager, State};

#[cfg(not(any(target_os = "android", target_os = "ios")))]
const DBX_HOST: &str = "127.0.0.1";
#[cfg(not(any(target_os = "android", target_os = "ios")))]
const DBX_PORT: u16 = 4224;

/// How long a remote probe (mobile connect form) may take before we give up.
const REMOTE_PROBE_TIMEOUT_MS: u64 = 3000;

#[cfg(not(any(target_os = "android", target_os = "ios")))]
const INSTALL_HINT: &str =
    "本机未检测到 dbx 命令。请确认 dbx 已加入系统 PATH，安装完成后点击重试。";

/// Tauri v2 -> v1 IPC shim, injected as a webview initialization script.
///
/// The dbx web UI (http://127.0.0.1:4224, a third-party page we cannot edit)
/// is written against the Tauri **v2** bridge (`window.__TAURI_INTERNALS__`),
/// but this shell is Tauri **v1**, which never defines that object. This shim
/// maps the v2 surface onto v1 (`window.__TAURI__.invoke`) and — crucially —
/// prefixes every failure with the command name, so the dbx UI's own error
/// dialog tells us exactly which commands it needs registered on the Rust side.
#[cfg(not(any(target_os = "android", target_os = "ios")))]
const TAURI_V2_SHIM: &str = r#"(function () {
  if (window.__TAURI_INTERNALS__) return;
  window.__TAURI_INTERNALS__ = {
    invoke: function (cmd, args) {
      args = args || {};
      if (!window.__TAURI__ || typeof window.__TAURI__.invoke !== 'function') {
        return Promise.reject(new Error('[dbx-shim] Tauri v1 bridge unavailable'));
      }
      return window.__TAURI__.invoke(cmd, args).catch(function (e) {
        var msg = e && e.message ? e.message : String(e);
        throw new Error('[dbx-shim] invoke("' + cmd + '") failed: ' + msg);
      });
    },
    transformCallback: function (cb) {
      return window.__TAURI__ && typeof window.__TAURI__.transformCallback === 'function'
        ? window.__TAURI__.transformCallback(cb)
        : cb;
    },
    convertFileSrc: function (p) {
      return window.__TAURI__ && typeof window.__TAURI__.convertFileSrc === 'function'
        ? window.__TAURI__.convertFileSrc(p)
        : p;
    }
  };
})();"#;


#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
#[cfg(target_os = "windows")]
const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;

#[cfg(not(any(target_os = "android", target_os = "ios")))]
struct DbxState {
    /// PID of the dbx instance this app spawned (None when the app did not
    /// start dbx, i.e. a user-started instance is being used).
    spawned: Mutex<Option<u32>>,
}

#[derive(Serialize)]
struct DbxStatus {
    running: bool,
    installed: bool,
    url: String,
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn dbx_url(port: Option<u16>) -> String {
    format!("http://{DBX_HOST}:{}", port.unwrap_or(DBX_PORT))
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn dbx_in_dir(dir: &std::path::Path) -> Option<PathBuf> {
    let base = dir.join("dbx");
    if base.is_file() {
        return Some(base);
    }
    #[cfg(windows)]
    {
        for ext in ["exe", "cmd", "bat", "com"] {
            let candidate = base.with_extension(ext);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// Candidate directories where `dbx` may be installed on macOS/Linux.
/// GUI-launched apps (Finder / `.desktop` / `open`) do not always inherit the
/// login shell's PATH, so we scan a set of common locations: npm/pnpm/bun
/// global bins, cargo, node version managers (nvm/volta/asdf/fnm/mise), and
/// the system bin dirs.
#[cfg(not(target_os = "windows"))]
fn common_bin_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![
        PathBuf::from("/usr/local/bin"),
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/bin"),
    ];
    if let Ok(home) = std::env::var("HOME") {
        let home = PathBuf::from(home);
        dirs.extend([
            home.join(".npm-global/bin"),
            home.join(".local/bin"),
            home.join(".cargo/bin"),
            home.join(".bun/bin"),
            home.join(".volta/bin"),
            home.join(".asdf/shims"),
            home.join(".local/share/mise/shims"),
            home.join(".local/share/fnm"),
        ]);
        // nvm keeps one `bin` dir per installed Node version, e.g.
        // ~/.nvm/versions/node/v20.11.0/bin.
        if let Ok(versions) = std::fs::read_dir(home.join(".nvm/versions/node")) {
            for entry in versions.flatten() {
                dirs.push(entry.path().join("bin"));
            }
        }
    }
    dirs
}

/// Resolve the `dbx` command so the app works on Windows/macOS/Linux no matter
/// how dbx was installed. Searches PATH first, then a set of common install
/// locations (GUI-launched apps on macOS/Linux do not always inherit the
/// login shell's PATH).
#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn find_dbx() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path) {
            if let Some(p) = dbx_in_dir(&dir) {
                return Some(p);
            }
        }
    }
    #[cfg(not(windows))]
    {
        for dir in common_bin_dirs() {
            if let Some(p) = dbx_in_dir(&dir) {
                return Some(p);
            }
        }
    }
    None
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn dbx_installed() -> bool {
    find_dbx().is_some()
}

/// True when a (lowercased) command-line token looks like the `dbx`
/// executable: the bare `dbx` or a `dbx`/`dbx.exe` binary invoked through its
/// absolute path.
#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn is_dbx_token(token: &str) -> bool {
    if token == "dbx" {
        return true;
    }
    if let Some(name) = std::path::Path::new(token)
        .file_name()
        .and_then(|f| f.to_str())
    {
        let stem = name
            .strip_suffix(".exe")
            .or_else(|| name.strip_suffix(".cmd"))
            .or_else(|| name.strip_suffix(".bat"))
            .or_else(|| name.strip_suffix(".com"))
            .unwrap_or(name);
        if stem == "dbx" {
            return true;
        }
    }
    token.contains(r"\dbx\")
        || token.contains("/dbx/")
        || token.contains(r"\dbx/")
        || token.contains(r"/dbx\")
}

/// Enumerate processes whose command line looks like a running `dbx` instance.
#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn find_dbx_processes() -> Vec<u32> {
    let mut sys = System::new();
    sys.refresh_processes_specifics(ProcessRefreshKind::new().with_cmd(UpdateKind::Always));
    let mut pids = Vec::new();
    for (pid, process) in sys.processes() {
        let cmd: Vec<String> = process.cmd().iter().map(|s| s.to_lowercase()).collect();
        if cmd.iter().any(|t| is_dbx_token(t)) {
            pids.push(pid.as_u32());
        }
    }
    pids.sort_unstable();
    pids
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn parse_port(addr: &str) -> Option<u16> {
    addr.rsplit(':').next()?.parse::<u16>().ok()
}

/// /proc/net/tcp uses hex-encoded ports (e.g. `0100007F:1080` -> 4224).
#[cfg(not(any(target_os = "android", target_os = "ios")))]
#[allow(dead_code)]
fn parse_hex_port(addr: &str) -> Option<u16> {
    u16::from_str_radix(addr.rsplit(':').next()?, 16).ok()
}

/// Parse `netstat -ano` output (Windows) and return the listening TCP ports
/// owned by `pid`.
#[cfg(not(any(target_os = "android", target_os = "ios")))]
#[allow(dead_code)]
fn ports_from_netstat(out: &str, pid: u32) -> Vec<u16> {
    let mut ports = Vec::new();
    for line in out.lines() {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() >= 5 && f[0].starts_with("TCP") && f[3] == "LISTENING" {
            if let Ok(p) = f[4].parse::<u32>() {
                if p == pid {
                    if let Some(port) = parse_port(f[1]) {
                        ports.push(port);
                    }
                }
            }
        }
    }
    ports
}

/// Parse `/proc/<pid>/net/tcp` (or tcp6) output (Linux). `inodes` is the set
/// of socket inodes held by the process's fds; only those rows are returned.
#[cfg(not(any(target_os = "android", target_os = "ios")))]
#[allow(dead_code)]
fn ports_from_proc_tcp(tcp: &str, inodes: &[u64]) -> Vec<u16> {
    let mut ports = Vec::new();
    for line in tcp.lines().skip(1) {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() >= 10 && f[3] == "0A" {
            if let Ok(inode) = f[9].parse::<u64>() {
                if inodes.contains(&inode) {
                    if let Some(port) = parse_hex_port(f[1]) {
                        ports.push(port);
                    }
                }
            }
        }
    }
    ports
}

/// Parse `lsof -nP -iTCP -sTCP:LISTEN -a -p <pid>` output (macOS) and return
/// the listening TCP ports owned by `pid`.
#[cfg(not(any(target_os = "android", target_os = "ios")))]
#[allow(dead_code)]
fn ports_from_lsof(out: &str, pid: u32) -> Vec<u16> {
    let mut ports = Vec::new();
    for line in out.lines() {
        let idx = match line.find("(LISTEN)") {
            Some(i) => i,
            None => continue,
        };
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() < 2 || f[1].parse::<u32>().ok() != Some(pid) {
            continue;
        }
        let before = &line[..idx];
        if let Some(addr) = before.trim_end().rsplit(' ').find(|s| !s.is_empty()) {
            if let Some(port) = parse_port(addr) {
                ports.push(port);
            }
        }
    }
    ports
}

/// Listening TCP ports owned by `pid`, discovered per platform.
#[cfg(not(any(target_os = "android", target_os = "ios")))]
#[allow(dead_code)]
fn listening_ports_of(pid: u32) -> Vec<u16> {
    #[cfg(windows)]
    {
        // CREATE_NO_WINDOW keeps netstat from flashing a console window on
        // every startup probe of this GUI app.
        let mut netstat = Command::new("netstat");
        netstat.args(["-ano"]).creation_flags(CREATE_NO_WINDOW);
        let out = netstat
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default();
        ports_from_netstat(&out, pid)
    }

    #[cfg(target_os = "linux")]
    {
        use std::fs;
        let mut inodes = Vec::new();
        if let Ok(entries) = fs::read_dir(format!("/proc/{pid}/fd")) {
            for entry in entries.flatten() {
                if let Ok(target) = fs::read_link(entry.path()) {
                    let s = target.to_string_lossy();
                    if let Some(num) = s.strip_prefix("socket:[").and_then(|r| r.strip_suffix(']'))
                    {
                        if let Ok(inode) = num.parse::<u64>() {
                            inodes.push(inode);
                        }
                    }
                }
            }
        }
        let mut ports = Vec::new();
        for file in [
            format!("/proc/{pid}/net/tcp"),
            format!("/proc/{pid}/net/tcp6"),
        ] {
            if let Ok(content) = fs::read_to_string(file) {
                ports.extend(ports_from_proc_tcp(&content, &inodes));
            }
        }
        ports
    }

    #[cfg(target_os = "macos")]
    {
        let out = Command::new("lsof")
            .args(["-nP", "-iTCP", "-sTCP:LISTEN", "-a", "-p", &pid.to_string()])
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default();
        ports_from_lsof(&out, pid)
    }

    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    {
        let _ = pid;
        Vec::new()
    }
}

fn parse_status(head: &str) -> Option<u16> {
    let line = head.lines().next()?;
    let mut parts = line.split_whitespace();
    parts.next()?; // HTTP/1.x
    parts.next()?.parse::<u16>().ok()
}

/// Resolve `host:port` into a concrete `SocketAddr`. Accepts IP literals
/// directly and falls back to DNS resolution for hostnames.
fn resolve_addr(host: &str, port: u16) -> Option<SocketAddr> {
    if let Ok(addr) = format!("{host}:{port}").parse::<SocketAddr>() {
        return Some(addr);
    }
    format!("{host}:{port}").to_socket_addrs().ok()?.next()
}

/// GET / on `host:port`; returns the HTTP status code and the first bytes of
/// the body if a response arrived.
fn fetch_head(host: &str, port: u16, timeout_ms: u64) -> Option<(u16, Vec<u8>)> {
    let addr = resolve_addr(host, port)?;
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_millis(timeout_ms)).ok()?;
    stream
        .set_read_timeout(Some(Duration::from_millis(timeout_ms)))
        .ok()?;
    let req = format!("GET / HTTP/1.0\r\nHost: {host}:{port}\r\n\r\n");
    stream.write_all(req.as_bytes()).ok()?;
    // Read until the headers plus a chunk of the body arrive, so a slow or
    // split first write cannot hide a valid HTTP response.
    let mut buf = Vec::with_capacity(16384);
    let mut chunk = [0u8; 4096];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                if buf.len() >= 16384 {
                    break;
                }
                if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                    if buf.len() - pos - 4 >= 1024 {
                        break;
                    }
                }
            }
            Err(e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                break;
            }
            Err(_) => break,
        }
    }
    if buf.is_empty() {
        return None;
    }
    let code = parse_status(&String::from_utf8_lossy(&buf))?;
    Some((code, buf))
}

/// True when `host:port` answers with an HTTP 2xx/3xx response. dbx owns a
/// fixed port, so a successful HTTP response there is sufficient to establish
/// readiness without depending on dbx's page title or HTML implementation.
fn is_dbx_server(host: &str, port: u16, timeout_ms: u64) -> bool {
    match fetch_head(host, port, timeout_ms) {
        Some((code, _body)) if (200..400).contains(&code) => true,
        _ => false,
    }
}

/// dbx always runs on the configured fixed port.
#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn discover_dbx() -> Option<u16> {
    is_dbx_server(DBX_HOST, DBX_PORT, 800).then_some(DBX_PORT)
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn spawn_dbx(_app: &AppHandle) -> Result<u32, String> {
    let dbx_path = find_dbx().ok_or_else(|| INSTALL_HINT.to_string())?;

    let log_dir = dirs::cache_dir()
        .ok_or_else(|| "无法解析系统缓存目录".to_string())?
        .join("dbx-desktop");
    std::fs::create_dir_all(&log_dir).map_err(|e| e.to_string())?;

    let log_path = log_dir.join("dbx.log");
    let log_file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .map_err(|e| format!("cannot open log {}: {e}", log_path.display()))?;

    // dbx uses port 4224 by default. Windows needs `cmd /C` to run a possible
    // npm shim (`dbx.cmd`); other platforms can execute the resolved command
    // directly.
    let (program, args): (String, Vec<String>) = if cfg!(windows) {
        let args = vec!["/C".into(), "dbx".into()];
        ("cmd".into(), args)
    } else {
        let args = Vec::new();
        (dbx_path.to_string_lossy().into_owned(), args)
    };

    let mut cmd = Command::new(&program);
    cmd.args(&args)
        .stdout(Stdio::from(
            log_file
                .try_clone()
                .map_err(|e| format!("cannot clone log handle: {e}"))?,
        ))
        .stderr(Stdio::from(log_file));

    // GUI-launched apps (Finder / `.desktop` / `open`) inherit a minimal PATH.
    // Prepend the dbx directory and all locations scanned above so auxiliary
    // executables remain discoverable.
    #[cfg(unix)]
    {
        let mut dirs: Vec<String> = Vec::new();
        if let Some(parent) = dbx_path.parent() {
            dirs.push(parent.to_string_lossy().into_owned());
        }
        dirs.extend(
            common_bin_dirs()
                .iter()
                .map(|d| d.to_string_lossy().into_owned()),
        );
        if let Ok(inherited) = std::env::var("PATH") {
            dirs.push(inherited);
        }
        cmd.env("PATH", dirs.join(":"));
    }

    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP);

    // Put dbx in its own process group so we can kill the whole tree on exit.
    #[cfg(unix)]
    cmd.process_group(0);

    let child = cmd
        .spawn()
        .map_err(|e| format!("failed to start `{program} {args:?}`: {e}"))?;
    Ok(child.id())
}

/// Kill the dbx instance this app spawned (the whole process tree). Used on
/// app exit; only called when the app started dbx itself.
#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn kill_dbx_tree(pid: u32) {
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .creation_flags(CREATE_NO_WINDOW)
            .status();
    }

    #[cfg(unix)]
    {
        // dbx was spawned with process_group(0), so its group id == pid.
        unsafe {
            libc::kill(-(pid as i32), libc::SIGTERM);
        }
        std::thread::sleep(Duration::from_millis(800));
        unsafe {
            libc::kill(-(pid as i32), libc::SIGKILL);
        }
    }

    #[cfg(not(any(windows, unix)))]
    {
        let _ = pid;
    }
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn ensure_started(app: &AppHandle, state: &DbxState) -> Result<Option<u32>, String> {
    if discover_dbx().is_some() {
        return Ok(None);
    }
    if !dbx_installed() {
        return Err(INSTALL_HINT.to_string());
    }
    // A dbx process exists but is not serving yet (still booting): wait for it
    // instead of spawning a duplicate on port 4224.
    if !find_dbx_processes().is_empty() {
        return Ok(None);
    }
    let mut spawned = state.spawned.lock().unwrap();
    if spawned.is_some() {
        return Ok(None);
    }
    let pid = spawn_dbx(app)?;
    *spawned = Some(pid);
    Ok(Some(pid))
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
#[tauri::command]
fn check_dbx() -> DbxStatus {
    let port = discover_dbx();
    DbxStatus {
        running: port.is_some(),
        installed: dbx_installed(),
        url: dbx_url(port),
    }
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
#[tauri::command]
fn start_dbx(app: AppHandle, state: State<'_, DbxState>) -> Result<DbxStatus, String> {
    let port = discover_dbx();
    if let Some(p) = port {
        return Ok(DbxStatus {
            running: true,
            installed: dbx_installed(),
            url: dbx_url(Some(p)),
        });
    }
    ensure_started(&app, &state)?;
    let p = discover_dbx();
    Ok(DbxStatus {
        running: p.is_some(),
        installed: dbx_installed(),
        url: dbx_url(p),
    })
}

/// Whether this build targets a mobile platform (Android/iOS). The frontend
/// uses it to pick between auto-starting local dbx (desktop) and asking for a
/// remote host:port (mobile).
#[tauri::command]
fn is_mobile() -> bool {
    cfg!(any(target_os = "android", target_os = "ios"))
}

/// Validate a user-supplied hostname/IP. Only bare hosts are accepted; the
/// port is a separate numeric field and the URL is assembled here so nothing
/// user-controlled can inject a scheme or path into it.
fn sanitize_host(raw: &str) -> Result<String, String> {
    let host = raw.trim();
    if host.is_empty() {
        return Err("请输入电脑的 IP 地址".into());
    }
    if host.len() > 253 {
        return Err("主机地址过长".into());
    }
    let invalid =
        |c: char| c.is_whitespace() || matches!(c, ':' | '/' | '\\' | '@' | '?' | '#' | '%');
    if host.chars().any(invalid) {
        return Err(
            "主机地址格式不正确，请只填写 IP 地址或主机名（端口请在单独的输入框填写）".into(),
        );
    }
    Ok(host.to_string())
}

/// Probe `http://host:port` and report whether it serves dbx. `running` is
/// false when the host is unreachable or does not respond to HTTP; the URL is
/// returned either way so the caller can still open it manually.
#[tauri::command]
fn connect_remote(host: String, port: u16) -> Result<DbxStatus, String> {
    let host = sanitize_host(&host)?;
    let url = format!("http://{host}:{port}");
    let running = is_dbx_server(&host, port, REMOTE_PROBE_TIMEOUT_MS);
    Ok(DbxStatus {
        running,
        installed: true,
        url,
    })
}

pub fn run() {
    let builder = tauri::Builder::default();

    // The single-instance / window-state plugins and the dbx process
    // management below are desktop-only; on mobile the app is a plain
    // remote-URL launcher.
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    let builder = builder
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .manage(DbxState {
            spawned: Mutex::new(None),
        });

    let app = builder
        .invoke_handler({
            #[cfg(not(any(target_os = "android", target_os = "ios")))]
            {
                tauri::generate_handler![check_dbx, start_dbx, connect_remote, is_mobile]
            }
            #[cfg(any(target_os = "android", target_os = "ios"))]
            {
                tauri::generate_handler![connect_remote, is_mobile]
            }
        })
        .setup(|app| {
            // Create the main window in Rust (not via tauri.conf.json) so we can
            // attach the Tauri v2 -> v1 IPC shim as an initialization script.
            // Init scripts run on EVERY page load of this webview, including the
            // remote dbx web UI (http://127.0.0.1:4224) we navigate to later.
            tauri::WindowBuilder::new(
                app,
                "main",
                tauri::WindowUrl::App("index.html".into()),
            )
            .title("DBX Desktop")
            .inner_size(800.0, 600.0)
            .min_inner_size(800.0, 600.0)
            .center()
            .initialization_script(TAURI_V2_SHIM)
            .build()
            .map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;

            #[cfg(not(any(target_os = "android", target_os = "ios")))]
            {
                let handle = app.handle().clone();
                std::thread::spawn(move || {
                    let state = handle.state::<DbxState>();
                    if let Err(e) = ensure_started(&handle, &state) {
                        eprintln!("dbx auto-start failed: {e}");
                    }
                });
            }
            #[cfg(any(target_os = "android", target_os = "ios"))]
            let _ = app;
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(|handle, event| {
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        if let tauri::RunEvent::Exit = event {
            // Only stop dbx when this app spawned it; a user-started instance
            // keeps running after the app closes.
            let spawned_pid = {
                let state = handle.state::<DbxState>();
                let guard = state.spawned.lock().unwrap();
                *guard
            };
            if let Some(pid) = spawned_pid {
                kill_dbx_tree(pid);
            }
        }
        #[cfg(any(target_os = "android", target_os = "ios"))]
        {
            let _ = (handle, event);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_port_basic() {
        assert_eq!(parse_port("127.0.0.1:4224"), Some(4224));
        assert_eq!(parse_port("[::1]:4232"), Some(4232));
        assert_eq!(parse_port("*:8090"), Some(8090));
        assert_eq!(parse_port("no-port"), None);
    }

    #[test]
    fn parse_status_basic() {
        assert_eq!(parse_status("HTTP/1.0 200 OK"), Some(200));
        assert_eq!(parse_status("HTTP/1.1 404 Not Found"), Some(404));
        assert_eq!(parse_status("garbage"), None);
    }

    #[test]
    fn netstat_windows_parse() {
        let sample = "\
Active Connections

  Proto  Local Address          Foreign Address        State           PID
  TCP    127.0.0.1:4224         0.0.0.0:0              LISTENING       38316
  TCP    127.0.0.1:4232         0.0.0.0:0              LISTENING       38316
  TCP    0.0.0.0:135            0.0.0.0:0              LISTENING       1204
  UDP    0.0.0.0:1900           *:*                                   544
";
        assert_eq!(ports_from_netstat(sample, 38316), vec![4224, 4232]);
        assert_eq!(ports_from_netstat(sample, 1204), vec![135]);
    }

    #[test]
    fn proc_tcp_linux_parse() {
        let sample = "\
  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 0100007F:1080 00000000:0000 0A 00000000:00000000 000:00000 00000000     0        0 12345 1 0000000000000000 100 0 0 10 0
   1: 0100007F:1088 00000000:0000 0A 00000000:00000000 000:00000 00000000     0        0 12346 1 0000000000000000 100 0 0 10 0
   2: 0100007F:D204 00000000:0000 01 00000000:00000000 000:00000 00000000     0        0 12347 1 0000000000000000 100 0 0 10 0
";
        assert_eq!(ports_from_proc_tcp(sample, &[12345]), vec![4224]);
        assert_eq!(ports_from_proc_tcp(sample, &[12346]), vec![4232]);
        // non-LISTEN row (state 01) must be ignored
        assert_eq!(ports_from_proc_tcp(sample, &[12347]), Vec::<u16>::new());
    }

    #[test]
    fn lsof_macos_parse() {
        let sample = "\
COMMAND PID   USER  FD   TYPE DEVICE SIZE/OFF NODE NAME
node    38316 cc    30u  IPv4 0x1   0t0     TCP 127.0.0.1:4224 (LISTEN)
node    38316 cc    31u  IPv6 0x2   0t0     TCP *:4232 (LISTEN)
node    999   cc    10u  IPv4 0x3   0t0     TCP 127.0.0.1:8443 (LISTEN)
";
        assert_eq!(ports_from_lsof(sample, 38316), vec![4224, 4232]);
        assert_eq!(ports_from_lsof(sample, 999), vec![8443]);
    }

    #[test]
    fn sanitize_host_basic() {
        assert_eq!(sanitize_host(" 192.168.1.10 "), Ok("192.168.1.10".into()));
        assert_eq!(sanitize_host("mypc.local"), Ok("mypc.local".into()));
        assert!(sanitize_host("").is_err());
        assert!(sanitize_host("   ").is_err());
        // scheme / path / port injection attempts are rejected
        assert!(sanitize_host("http://192.168.1.10").is_err());
        assert!(sanitize_host("192.168.1.10:4224").is_err());
        assert!(sanitize_host("192.168.1.10/x").is_err());
        assert!(sanitize_host(r"192.168.1.10\dbx").is_err());
        assert!(sanitize_host("user@host").is_err());
    }

    #[test]
    fn dbx_token_matches() {
        assert!(is_dbx_token("dbx"));
        assert!(is_dbx_token("/usr/local/bin/dbx"));
        assert!(is_dbx_token("/home/me/.local/bin/dbx"));
        assert!(is_dbx_token("dbx.exe"));
        assert!(is_dbx_token("C:\\tools\\dbx\\dbx.exe"));
        assert!(!is_dbx_token("node"));
        assert!(!is_dbx_token("/usr/bin/bash"));
        assert!(!is_dbx_token("--webview-exe-name=msedgewebview2.exe"));
        assert!(!is_dbx_token("/opt/dashboard/bin/web"));
    }
}
