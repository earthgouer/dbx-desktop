const { invoke } = window.__TAURI__.core;

const url = "http://127.0.0.1:3080";
const statusEl = document.getElementById("status");
const spinnerEl = document.getElementById("spinner");
const urlEl = document.getElementById("url");
const errorBox = document.getElementById("error");
const errorText = document.getElementById("error-text");
const retryBtn = document.getElementById("retry");

function setStatus(text, busy) {
  statusEl.textContent = text;
  spinnerEl.hidden = !busy;
}

function showError(text) {
  errorText.textContent = text;
  errorBox.hidden = false;
  spinnerEl.hidden = true;
}

async function probe() {
  try {
    const s = await invoke("check_dsh");
    return s.running === true;
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

function open(url) {
  setStatus("dsh 已就绪，正在打开界面…", true);
  urlEl.hidden = false;
  urlEl.textContent = "正在打开：" + url;
  setTimeout(() => {
    window.location.replace(url);
  }, 300);
}

async function boot() {
  setStatus("正在检测 dsh…", true);
  errorBox.hidden = true;

  let status;
  try {
    status = await invoke("check_dsh");
  } catch (e) {
    showError("检测失败：" + (e && e.message ? e.message : e));
    return;
  }

  if (status.running) {
    open(url);
    return;
  }

  if (!status.installed) {
    showError("本机未检测到 dsh 命令。请先安装 dsh：npm install -g @deepseek-ai/dsh，安装完成后点击重试。");
    return;
  }

  setStatus("正在启动 dsh…", true);
  try {
    await invoke("start_dsh");
  } catch (e) {
    const msg = typeof e === "string" ? e : e && e.message ? e.message : String(e);
    showError(msg);
    return;
  }

  if (await waitReady(90000)) {
    open(url);
  } else {
    showError("dsh 启动超时，请检查日志后重试。");
  }
}

retryBtn.addEventListener("click", boot);
boot();
