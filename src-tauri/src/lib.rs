use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::Duration;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[cfg(unix)]
use std::os::unix::process::CommandExt;

use serde::Serialize;
use sysinfo::{ProcessRefreshKind, System, UpdateKind};
use tauri::{AppHandle, Manager, State};

const DSH_HOST: &str = "127.0.0.1";
const DSH_PORT: u16 = 3080;

const INSTALL_HINT: &str =
    "本机未检测到 dsh 命令。请先安装 dsh（npm install -g @deepseek-ai/dsh），安装完成后点击重试。";

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
#[cfg(windows)]
const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;

struct DshState {
    /// PID of the dsh instance this app spawned (None when the app did not
    /// start dsh, i.e. a user-started instance is being used).
    spawned: Mutex<Option<u32>>,
}

#[derive(Serialize)]
struct DshStatus {
    running: bool,
    installed: bool,
    url: String,
}

fn dsh_url(port: Option<u16>) -> String {
    format!("http://{DSH_HOST}:{}", port.unwrap_or(DSH_PORT))
}

fn dsh_in_dir(dir: &std::path::Path) -> Option<PathBuf> {
    let base = dir.join("dsh");
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

/// Candidate directories where `dsh` may be installed on macOS/Linux.
/// GUI-launched apps (Finder / `.desktop` / `open`) do not always inherit the
/// login shell's PATH, so we scan a set of common locations: npm/pnpm/bun
/// global bins, cargo, node version managers (nvm/volta/asdf/fnm/mise), and
/// the system bin dirs.
#[cfg(not(windows))]
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

/// Resolve the `dsh` command so the app works on Windows/macOS/Linux no matter
/// how dsh was installed. Searches PATH first, then a set of common install
/// locations (GUI-launched apps on macOS/Linux do not always inherit the
/// login shell's PATH).
fn find_dsh() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path) {
            if let Some(p) = dsh_in_dir(&dir) {
                return Some(p);
            }
        }
    }
    #[cfg(not(windows))]
    {
        for dir in common_bin_dirs() {
            if let Some(p) = dsh_in_dir(&dir) {
                return Some(p);
            }
        }
    }
    None
}

fn dsh_installed() -> bool {
    find_dsh().is_some()
}

/// True when a (lowercased) command-line token looks like the `dsh`
/// executable: the bare `dsh`, a `dsh`/`dsh.exe` binary invoked through its
/// absolute path (e.g. `/usr/local/bin/dsh`, `~/.cargo/bin/dsh`), or a node
/// script under the `@deepseek-ai/dsh` package.
fn is_dsh_token(token: &str) -> bool {
    if token == "dsh" {
        return true;
    }
    if let Some(name) = std::path::Path::new(token).file_name().and_then(|f| f.to_str()) {
        let stem = name
            .strip_suffix(".exe")
            .or_else(|| name.strip_suffix(".cmd"))
            .or_else(|| name.strip_suffix(".bat"))
            .or_else(|| name.strip_suffix(".com"))
            .unwrap_or(name);
        if stem == "dsh" {
            return true;
        }
    }
    token.contains("@deepseek-ai/dsh")
        || token.contains(r"\dsh\")
        || token.contains("/dsh/")
        || token.contains(r"\dsh/")
        || token.contains(r"/dsh\")
}

/// Enumerate processes whose command line looks like a running `dsh web`
/// instance (e.g. `node .../@deepseek-ai/dsh/lib/bin.js web --port 8080`).
/// This is what lets the app find a dsh that the user started on any port.
/// Matching is strict on purpose: a bare token "web" plus a dsh-looking path
/// or command, so the app's own WebView2 processes (`--webview-exe-name=...`)
/// never match.
fn find_dsh_processes() -> Vec<u32> {
    let mut sys = System::new();
    sys.refresh_processes_specifics(ProcessRefreshKind::new().with_cmd(UpdateKind::Always));
    let mut pids = Vec::new();
    for (pid, process) in sys.processes() {
        let cmd: Vec<String> = process.cmd().iter().map(|s| s.to_lowercase()).collect();
        let is_dsh = cmd.iter().any(|t| is_dsh_token(t));
        let has_web = cmd.iter().any(|t| t == "web");
        if is_dsh && has_web {
            pids.push(pid.as_u32());
        }
    }
    pids.sort_unstable();
    pids
}

fn parse_port(addr: &str) -> Option<u16> {
    addr.rsplit(':').next()?.parse::<u16>().ok()
}

/// /proc/net/tcp uses hex-encoded ports (e.g. `0100007F:0C08` -> 3080).
#[allow(dead_code)]
fn parse_hex_port(addr: &str) -> Option<u16> {
    u16::from_str_radix(addr.rsplit(':').next()?, 16).ok()
}

/// Parse `netstat -ano` output (Windows) and return the listening TCP ports
/// owned by `pid`.
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
fn listening_ports_of(pid: u32) -> Vec<u16> {
    #[cfg(windows)]
    {
        let out = Command::new("netstat")
            .args(["-ano"])
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
                    if let Some(num) = s
                        .strip_prefix("socket:[")
                        .and_then(|r| r.strip_suffix(']'))
                    {
                        if let Ok(inode) = num.parse::<u64>() {
                            inodes.push(inode);
                        }
                    }
                }
            }
        }
        let mut ports = Vec::new();
        for file in [format!("/proc/{pid}/net/tcp"), format!("/proc/{pid}/net/tcp6")] {
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

/// GET / on `port`; returns the HTTP status code and the first bytes of the
/// body if a response arrived.
fn fetch_head(port: u16) -> Option<(u16, Vec<u8>)> {
    let addr: SocketAddr = format!("{DSH_HOST}:{port}").parse().ok()?;
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_millis(800)).ok()?;
    stream.set_read_timeout(Some(Duration::from_millis(800))).ok()?;
    let req = format!("GET / HTTP/1.0\r\nHost: {DSH_HOST}:{port}\r\n\r\n");
    stream.write_all(req.as_bytes()).ok()?;
    // Read until the headers plus a chunk of the body arrive, so a slow or
    // split first write can't hide the "DeepSeek/Harness" markers.
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

/// True when `port` answers with an HTTP 2xx/3xx page that looks like the dsh
/// web UI (DeepSeek Harness).
fn is_dsh_server(port: u16) -> bool {
    match fetch_head(port) {
        Some((code, body)) if (200..400).contains(&code) => {
            let text = String::from_utf8_lossy(&body).to_lowercase();
            text.contains("deepseek") || text.contains("harness")
        }
        _ => false,
    }
}

/// Resolve the port of a running dsh web instance. Prefers the default port
/// (3080), then any port a running dsh process is serving. Returns `None`
/// when nothing is serving dsh.
fn discover_dsh() -> Option<u16> {
    if is_dsh_server(DSH_PORT) {
        return Some(DSH_PORT);
    }
    for pid in find_dsh_processes() {
        for port in listening_ports_of(pid) {
            if port != DSH_PORT && is_dsh_server(port) {
                return Some(port);
            }
        }
    }
    None
}

fn spawn_dsh(app: &AppHandle) -> Result<u32, String> {
    let dsh_path = find_dsh().ok_or_else(|| INSTALL_HINT.to_string())?;

    let log_dir = app.path().app_log_dir().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&log_dir).map_err(|e| e.to_string())?;

    let log_path = log_dir.join("dsh-web.log");
    let log_file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .map_err(|e| format!("cannot open log {}: {e}", log_path.display()))?;

    // Windows needs `cmd /C` to run the npm shim (dsh.cmd); other platforms
    // can exec the resolved `dsh` directly (binary or shebang script).
    let (program, args): (String, Vec<String>) = if cfg!(windows) {
        (
            "cmd".into(),
            vec![
                "/C".into(),
                "dsh".into(),
                "web".into(),
                "--port".into(),
                DSH_PORT.to_string(),
            ],
        )
    } else {
        (
            dsh_path.to_string_lossy().into_owned(),
            vec!["web".into(), "--port".into(), DSH_PORT.to_string()],
        )
    };

    let mut cmd = Command::new(&program);
    cmd.args(&args)
        .stdout(Stdio::from(
            log_file
                .try_clone()
                .map_err(|e| format!("cannot clone log handle: {e}"))?,
        ))
        .stderr(Stdio::from(log_file));

    // GUI-launched apps (Finder / `.desktop` / `open`) inherit a minimal PATH,
    // so dsh's shebang (`#!/usr/bin/env node`) can fail to locate node.
    // Prepend the dir holding dsh (npm/nvm/pnpm keep node next to dsh there)
    // plus every directory we search for dsh to the inherited PATH.
    #[cfg(unix)]
    {
        let mut dirs: Vec<String> = Vec::new();
        if let Some(parent) = dsh_path.parent() {
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

    // Put dsh in its own process group so we can kill the whole tree on exit.
    #[cfg(unix)]
    cmd.process_group(0);

    let child = cmd
        .spawn()
        .map_err(|e| format!("failed to start `{program} {args:?}`: {e}"))?;
    Ok(child.id())
}

/// Kill the dsh instance this app spawned (the whole process tree). Used on
/// app exit; only called when the app started dsh itself.
fn kill_dsh_tree(pid: u32) {
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .creation_flags(CREATE_NO_WINDOW)
            .status();
    }

    #[cfg(unix)]
    {
        // dsh was spawned with process_group(0), so its group id == pid.
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

fn ensure_started(app: &AppHandle, state: &DshState) -> Result<Option<u32>, String> {
    if discover_dsh().is_some() {
        return Ok(None);
    }
    if !dsh_installed() {
        return Err(INSTALL_HINT.to_string());
    }
    // A dsh process exists but is not serving yet (still booting): wait for it
    // instead of spawning a duplicate on port 3080.
    if !find_dsh_processes().is_empty() {
        return Ok(None);
    }
    let mut spawned = state.spawned.lock().unwrap();
    if spawned.is_some() {
        return Ok(None);
    }
    let pid = spawn_dsh(app)?;
    *spawned = Some(pid);
    Ok(Some(pid))
}

#[tauri::command]
fn check_dsh() -> DshStatus {
    let port = discover_dsh();
    DshStatus {
        running: port.is_some(),
        installed: dsh_installed(),
        url: dsh_url(port),
    }
}

#[tauri::command]
fn start_dsh(app: AppHandle, state: State<'_, DshState>) -> Result<DshStatus, String> {
    let port = discover_dsh();
    if let Some(p) = port {
        return Ok(DshStatus {
            running: true,
            installed: dsh_installed(),
            url: dsh_url(Some(p)),
        });
    }
    ensure_started(&app, &state)?;
    let p = discover_dsh();
    Ok(DshStatus {
        running: p.is_some(),
        installed: dsh_installed(),
        url: dsh_url(p),
    })
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(win) = app.get_webview_window("main") {
                let _ = win.set_focus();
            }
        }))
        .manage(DshState {
            spawned: Mutex::new(None),
        })
        .invoke_handler(tauri::generate_handler![check_dsh, start_dsh])
        .setup(|app| {
            let handle = app.handle().clone();
            std::thread::spawn(move || {
                let state = handle.state::<DshState>();
                if let Err(e) = ensure_started(&handle, &state) {
                    eprintln!("dsh auto-start failed: {e}");
                }
            });
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(|handle, event| {
        if let tauri::RunEvent::Exit = event {
            // Only stop dsh when this app spawned it; a user-started instance
            // keeps running after the app closes.
            let spawned_pid = {
                let state = handle.state::<DshState>();
                let guard = state.spawned.lock().unwrap();
                *guard
            };
            if let Some(pid) = spawned_pid {
                kill_dsh_tree(pid);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_port_basic() {
        assert_eq!(parse_port("127.0.0.1:3080"), Some(3080));
        assert_eq!(parse_port("[::1]:3088"), Some(3088));
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
  TCP    127.0.0.1:3080         0.0.0.0:0              LISTENING       38316
  TCP    127.0.0.1:3088         0.0.0.0:0              LISTENING       38316
  TCP    0.0.0.0:135            0.0.0.0:0              LISTENING       1204
  UDP    0.0.0.0:1900           *:*                                   544
";
        assert_eq!(ports_from_netstat(sample, 38316), vec![3080, 3088]);
        assert_eq!(ports_from_netstat(sample, 1204), vec![135]);
    }

    #[test]
    fn proc_tcp_linux_parse() {
        let sample = "\
  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 0100007F:0C08 00000000:0000 0A 00000000:00000000 000:00000 00000000     0        0 12345 1 0000000000000000 100 0 0 10 0
   1: 0100007F:0C10 00000000:0000 0A 00000000:00000000 000:00000 00000000     0        0 12346 1 0000000000000000 100 0 0 10 0
   2: 0100007F:D204 00000000:0000 01 00000000:00000000 000:00000 00000000     0        0 12347 1 0000000000000000 100 0 0 10 0
";
        assert_eq!(ports_from_proc_tcp(sample, &[12345]), vec![3080]);
        assert_eq!(ports_from_proc_tcp(sample, &[12346]), vec![3088]);
        // non-LISTEN row (state 01) must be ignored
        assert_eq!(ports_from_proc_tcp(sample, &[12347]), Vec::<u16>::new());
    }

    #[test]
    fn lsof_macos_parse() {
        let sample = "\
COMMAND PID   USER  FD   TYPE DEVICE SIZE/OFF NODE NAME
node    38316 cc    30u  IPv4 0x1   0t0     TCP 127.0.0.1:3080 (LISTEN)
node    38316 cc    31u  IPv6 0x2   0t0     TCP *:3088 (LISTEN)
node    999   cc    10u  IPv4 0x3   0t0     TCP 127.0.0.1:8443 (LISTEN)
";
        assert_eq!(ports_from_lsof(sample, 38316), vec![3080, 3088]);
        assert_eq!(ports_from_lsof(sample, 999), vec![8443]);
    }

    #[test]
    fn dsh_token_matches() {
        assert!(is_dsh_token("dsh"));
        assert!(is_dsh_token("/usr/local/bin/dsh"));
        assert!(is_dsh_token("/home/me/.cargo/bin/dsh"));
        assert!(is_dsh_token("dsh.exe"));
        assert!(is_dsh_token("C:\\tools\\dsh\\dsh.exe"));
        assert!(is_dsh_token("/usr/lib/node_modules/@deepseek-ai/dsh/lib/bin.js"));
        assert!(!is_dsh_token("node"));
        assert!(!is_dsh_token("/usr/bin/bash"));
        assert!(!is_dsh_token("--webview-exe-name=msedgewebview2.exe"));
        assert!(!is_dsh_token("/opt/dashboard/bin/web"));
    }
}
