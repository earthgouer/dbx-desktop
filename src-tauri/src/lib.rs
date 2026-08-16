use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::Duration;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

use serde::Serialize;
use tauri::{AppHandle, Manager, State};

const DSH_HOST: &str = "127.0.0.1";
const DSH_PORT: u16 = 3080;
const DSH_URL: &str = "http://127.0.0.1:3080";

const INSTALL_HINT: &str =
    "本机未检测到 dsh 命令。请先安装 dsh（npm install -g @deepseek-ai/dsh），安装完成后点击重试。";

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
#[cfg(windows)]
const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;

struct DshState {
    spawned: Mutex<bool>,
}

#[derive(Serialize)]
struct DshStatus {
    running: bool,
    installed: bool,
    url: String,
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

/// Resolve the `dsh` command so the app works on Windows/macOS/Linux no matter
/// how dsh was installed. Searches PATH first, then a few common install
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
        if let Ok(home) = std::env::var("HOME") {
            let home = PathBuf::from(home);
            let candidates = [
                home.join(".npm-global/bin"),
                home.join(".local/bin"),
                home.join(".bun/bin"),
                PathBuf::from("/usr/local/bin"),
                PathBuf::from("/opt/homebrew/bin"),
                PathBuf::from("/usr/bin"),
            ];
            for dir in candidates {
                if let Some(p) = dsh_in_dir(&dir) {
                    return Some(p);
                }
            }
        }
    }
    None
}

fn dsh_installed() -> bool {
    find_dsh().is_some()
}

fn probe() -> bool {
    let addr: SocketAddr = match format!("{DSH_HOST}:{DSH_PORT}").parse() {
        Ok(a) => a,
        Err(_) => return false,
    };
    let mut stream = match TcpStream::connect_timeout(&addr, Duration::from_millis(800)) {
        Ok(s) => s,
        Err(_) => return false,
    };
    let _ = stream.set_read_timeout(Some(Duration::from_millis(1500)));
    let req = format!("GET / HTTP/1.0\r\nHost: {DSH_HOST}:{DSH_PORT}\r\n\r\n");
    if stream.write_all(req.as_bytes()).is_err() {
        return false;
    }
    let mut buf = [0u8; 4096];
    match stream.read(&mut buf) {
        Ok(n) => {
            let head = String::from_utf8_lossy(&buf[..n]);
            parse_status(&head).is_some_and(|code| (200..400).contains(&code))
        }
        Err(_) => false,
    }
}

fn parse_status(head: &str) -> Option<u16> {
    let line = head.lines().next()?;
    let mut parts = line.split_whitespace();
    parts.next()?; // HTTP/1.x
    parts.next()?.parse::<u16>().ok()
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

    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP);

    let child = cmd
        .spawn()
        .map_err(|e| format!("failed to start `{program} {args:?}`: {e}"))?;
    Ok(child.id())
}

fn ensure_started(app: &AppHandle, state: &DshState) -> Result<Option<u32>, String> {
    if probe() {
        return Ok(None);
    }
    if !dsh_installed() {
        return Err(INSTALL_HINT.to_string());
    }
    let mut spawned = state.spawned.lock().unwrap();
    if *spawned {
        return Ok(None);
    }
    let pid = spawn_dsh(app)?;
    *spawned = true;
    Ok(Some(pid))
}

#[tauri::command]
fn check_dsh() -> DshStatus {
    DshStatus {
        running: probe(),
        installed: dsh_installed(),
        url: DSH_URL.to_string(),
    }
}

#[tauri::command]
fn start_dsh(app: AppHandle, state: State<'_, DshState>) -> Result<DshStatus, String> {
    if probe() {
        return Ok(DshStatus {
            running: true,
            installed: dsh_installed(),
            url: DSH_URL.to_string(),
        });
    }
    ensure_started(&app, &state)?;
    Ok(DshStatus {
        running: false,
        installed: dsh_installed(),
        url: DSH_URL.to_string(),
    })
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(DshState {
            spawned: Mutex::new(false),
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
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
