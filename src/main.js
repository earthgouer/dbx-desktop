const { invoke } = window.__TAURI__;

const titleEl = document.getElementById("title");
const statusEl = document.getElementById("status");
const spinnerEl = document.getElementById("spinner");
const urlEl = document.getElementById("url");
const errorBox = document.getElementById("error");
const errorText = document.getElementById("error-text");
const retryBtn = document.getElementById("retry");

const remoteFlowEl = document.getElementById("remote-flow");
const autoFlowEl = document.getElementById("auto-flow");
const connectForm = document.getElementById("connect-form");
const hostInput = document.getElementById("host-input");
const portInput = document.getElementById("port-input");
const connectBtn = document.getElementById("connect-btn");
const remoteStatusEl = document.getElementById("remote-status");
const remoteSpinnerEl = document.getElementById("remote-spinner");

const STORAGE_KEY = "dbx.remote";

let lastStatus = null;
let connecting = false;
let mobileMode = false;

function setStatus(text, busy) {
  statusEl.textContent = text;
  spinnerEl.hidden = !busy;
}

function showError(text) {
  errorText.textContent = text;
  errorBox.hidden = false;
  spinnerEl.hidden = true;
  remoteSpinnerEl.hidden = true;
}

/* ---------- 桌面端：自动检测 / 拉起本机 dbx ---------- */

async function probe() {
  try {
    lastStatus = await invoke("check_dbx");
    return lastStatus.running === true;
  } catch {
    return false;
  }
}

async function waitReady(timeoutMs) {
  const started = Date.now();
  while (Date.now() - started < timeoutMs) {
    if (await probe()) return true;
    await new Promise((r) => setTimeout(r, 1000));
  }
  return false;
}

function open(u) {
  setStatus("dbx 已就绪，正在打开界面…", true);
  urlEl.hidden = false;
  urlEl.textContent = "正在打开：" + u;
  setTimeout(() => {
    window.location.replace(u);
  }, 300);
}

async function bootDesktop() {
  setStatus("正在检测 dbx…", true);
  errorBox.hidden = true;

  let status;
  try {
    status = await invoke("check_dbx");
  } catch (e) {
    showError("检测失败：" + (e && e.message ? e.message : e));
    return;
  }

  if (status.running) {
    open(status.url);
    return;
  }

  if (!status.installed) {
    showError("本机未检测到 dbx 命令。请确认 dbx 已加入系统 PATH，安装完成后点击重试。");
    return;
  }

  setStatus("正在启动 dbx（端口 4224）…", true);
  try {
    await invoke("start_dbx");
  } catch (e) {
    const msg = typeof e === "string" ? e : e && e.message ? e.message : String(e);
    showError(msg);
    return;
  }

  if (await waitReady(90000)) {
    open(lastStatus.url);
  } else {
    showError("dbx 启动超时，请检查日志后重试。");
  }
}

/* ---------- 移动端：连接局域网内已运行的 dbx ---------- */

function loadSavedRemote() {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return null;
    const saved = JSON.parse(raw);
    if (!saved || typeof saved.host !== "string" || !saved.host) return null;
    const port = Number(saved.port);
    if (!Number.isInteger(port) || port < 1 || port > 65535) return null;
    return { host: saved.host, port };
  } catch {
    return null;
  }
}

function saveRemote(host, port) {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify({ host, port }));
  } catch {}
}

function setConnecting(busy) {
  connecting = busy;
  connectBtn.disabled = busy;
  hostInput.disabled = busy;
  portInput.disabled = busy;
  remoteSpinnerEl.hidden = !busy;
  if (busy) {
    remoteStatusEl.hidden = false;
    remoteStatusEl.textContent = "正在连接…";
    errorBox.hidden = true;
  } else {
    remoteStatusEl.hidden = true;
    remoteStatusEl.textContent = "";
  }
}

function openRemote(u) {
  connecting = true;
  connectBtn.disabled = true;
  remoteSpinnerEl.hidden = false;
  remoteStatusEl.hidden = false;
  remoteStatusEl.textContent = "dbx 已就绪，正在打开界面…";
  setTimeout(() => {
    window.location.replace(u);
  }, 300);
}

async function connectRemote(host, port) {
  if (connecting) return;
  setConnecting(true);
  try {
    const st = await invoke("connect_remote", { host, port });
    if (st.running) {
      saveRemote(host, port);
      openRemote(st.url);
    } else {
      showError(
        `${host}:${port} 未检测到 dbx 服务。请确认电脑上的 dbx 已启动、手机与电脑在同一网络，且地址和端口填写正确。`
      );
      setConnecting(false);
    }
  } catch (e) {
    showError(typeof e === "string" ? e : "连接失败：" + (e && e.message ? e.message : e));
    setConnecting(false);
  }
}

function bootMobile() {
  mobileMode = true;
  titleEl.textContent = "DBX";
  autoFlowEl.hidden = true;
  errorBox.hidden = true;
  remoteFlowEl.hidden = false;

  const saved = loadSavedRemote();
  if (saved) {
    hostInput.value = saved.host;
    portInput.value = saved.port;
    connectRemote(saved.host, saved.port);
  }
}

connectForm.addEventListener("submit", (ev) => {
  ev.preventDefault();
  if (connecting) return;
  const host = hostInput.value.trim();
  const port = Number(portInput.value);
  if (!host) {
    showError("请输入电脑的 IP 地址。");
    return;
  }
  if (!/^[a-zA-Z0-9.\-_]+$/.test(host)) {
    showError("IP 地址格式不正确（示例：192.168.1.100）。");
    return;
  }
  if (!Number.isInteger(port) || port < 1 || port > 65535) {
    showError("端口需为 1-65535 之间的数字。");
    return;
  }
  errorBox.hidden = true;
  connectRemote(host, port);
});

retryBtn.addEventListener("click", () => {
  if (!mobileMode) {
    bootDesktop();
    return;
  }
  // 移动端：用当前表单里的地址重试
  const host = hostInput.value.trim();
  const port = Number(portInput.value);
  errorBox.hidden = true;
  if (host && Number.isInteger(port) && port >= 1 && port <= 65535) {
    connectRemote(host, port);
  }
});

/* ---------- 入口 ---------- */

async function boot() {
  let mobile = false;
  try {
    mobile = await invoke("is_mobile");
  } catch {
    // 兜底：命令不可用时按 UA 粗略判断
    mobile = /Android|iPhone|iPad|iPod/i.test(navigator.userAgent);
  }
  if (mobile) {
    bootMobile();
  } else {
    bootDesktop();
  }
}

boot();
