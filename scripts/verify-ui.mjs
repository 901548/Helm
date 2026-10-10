// CDP UI 冒烟：连本机 WebView2 调试端口，按「命名检查」逐项断言并汇总。
//
// 用法：node scripts/verify-ui.mjs [port]        （默认 9229）
// 退出码：全部通过 0，否则 1。
//
// 启动被测应用（必须带调试端口环境变量，否则 9229 永不监听）：
//   powershell -NoProfile -Command "$env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS='--remote-debugging-port=9229'; Start-Process -FilePath 'F:\Helm\src-tauri\target\debug\helm.exe' -WorkingDirectory 'F:\Helm'"
// 注：debug 构建按 devUrl 载入前端，需先起 vite（`npx vite --host 127.0.0.1`）。

const port = process.argv[2] ?? "9229";

const targets = await (await fetch(`http://127.0.0.1:${port}/json`)).json();
const page = targets.find((t) => t.type === "page" && t.webSocketDebuggerUrl);
if (!page) {
  console.error(`未找到可调试页面（端口 ${port}）`);
  process.exit(1);
}

const ws = new WebSocket(page.webSocketDebuggerUrl);
let msgId = 0;
const pending = new Map();
/** 运行期收集的未捕获异常 / console.error */
const pageIssues = [];

const send = (method, params = {}) =>
  new Promise((res, rej) => {
    const mid = ++msgId;
    pending.set(mid, { res, rej });
    ws.send(JSON.stringify({ id: mid, method, params }));
  });

ws.onmessage = (ev) => {
  const msg = JSON.parse(ev.data);
  if (msg.id && pending.has(msg.id)) {
    const { res, rej } = pending.get(msg.id);
    pending.delete(msg.id);
    msg.error ? rej(new Error(msg.error.message)) : res(msg.result);
    return;
  }
  if (msg.method === "Runtime.exceptionThrown") {
    pageIssues.push("exception: " + (msg.params.exceptionDetails?.exception?.description ?? "?"));
  }
  if (msg.method === "Log.entryAdded" && msg.params.entry.level === "error") {
    pageIssues.push("log: " + msg.params.entry.text);
  }
};

await new Promise((r) => (ws.onopen = r));
await send("Runtime.enable");
await send("Log.enable");

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

/** 求值并取回可序列化结果；表达式抛错即失败 */
async function evalJson(expr) {
  const r = await send("Runtime.evaluate", {
    expression: expr,
    returnByValue: true,
    awaitPromise: true,
  });
  if (r.exceptionDetails) {
    throw new Error(r.exceptionDetails.exception?.description ?? r.exceptionDetails.text);
  }
  return r.result.value;
}

const results = [];
function check(name, ok, detail = "") {
  results.push({ name, ok });
  console.log(`${ok ? "PASS" : "FAIL"}  ${name}${detail ? "   — " + detail : ""}`);
}
/** 轮询直到表达式为真（Svelte 更新是异步的，同步直查必失） */
async function waitFor(expr, timeoutMs = 8000, stepMs = 200) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (await evalJson(expr)) return true;
    await sleep(stepMs);
  }
  return false;
}
const click = (sel) => evalJson(`(() => { const e = document.querySelector(${JSON.stringify(sel)}); if (!e) return false; e.click(); return true; })()`);
const pressEsc = () => evalJson(`(() => { window.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape' })); return true; })()`);
/** Esc 关闭后轮询等 DOM 消失（Svelte 状态更新是异步的，立即读必拿到旧 DOM） */
const escCloses = async (labelledBy) =>
  pressEsc() &&
  (await waitFor(`document.querySelector('[role=dialog][aria-labelledby=${labelledBy}]') === null`, 4000, 100));

// ---------- 1. 基础 ----------
const mount = await evalJson(`JSON.stringify({ app: document.getElementById('app')?.innerHTML.length ?? -1, dialogs: document.querySelectorAll('[role=dialog]').length })`);
check("应用已挂载且初始无弹窗", JSON.parse(mount).app > 1000 && JSON.parse(mount).dialogs === 0, mount);

// favicon（P101-7）
const fav = await evalJson(`(async () => { const l = document.querySelector('link[rel=icon]'); if (!l) return 'no-link'; const r = await fetch(l.href); return r.status + ':' + (r.headers.get('content-type') || ''); })()`);
check("favicon 声明存在且可取回 200", String(fav).startsWith("200"), fav);

// ---------- 2. 会话表单弹窗（P101-1/2） ----------
const opened = await click('button[title="新建会话"]');
await waitFor(`document.querySelector('[role=dialog][aria-labelledby=sf-title]') !== null`, 4000, 100);
const sf = await evalJson(`JSON.stringify((() => { const d = document.querySelector('[role=dialog][aria-labelledby=sf-title]'); if (!d) return null; return { modal: d.getAttribute('aria-modal'), focus: document.activeElement === d, closeTitle: d.querySelector('.close')?.title }; })())`);
check("会话表单：dialog 语义 + 挂载聚焦 + 关闭按钮标注 Esc", opened === true && sf !== "null" && JSON.parse(sf).modal === "true" && JSON.parse(sf).focus && /Esc/.test(JSON.parse(sf).closeTitle ?? ""), sf);
check("会话表单：Esc 关闭", await escCloses("sf-title"));

// ---------- 3. 历史面板（P101-1/2/6） ----------
const hOpen = await click('button[title="操作历史（训练数据）"]');
const hReady = await waitFor(`document.querySelectorAll('.rec').length > 0`, 10000);
const hist = await evalJson(`JSON.stringify((() => { const d = document.querySelector('[role=dialog][aria-labelledby=hist-title]'); return d ? { modal: d.getAttribute('aria-modal'), focus: document.activeElement === d, rows: document.querySelectorAll('.rec').length } : null; })())`);
check("历史面板：dialog 语义 + 记录加载", hOpen === true && hReady && hist !== "null" && JSON.parse(hist).modal === "true" && JSON.parse(hist).rows > 0, hist);

// 展开稳定：点第 2 行 → 记录展开项文本 → 切换类型筛选 → 展开项要么消失、要么仍是同一条
await evalJson(`(() => { const rows = [...document.querySelectorAll('.rec')]; (rows[1] ?? rows[0])?.click(); return true; })()`);
await sleep(250);
const ex = await evalJson(`JSON.stringify({ open: document.querySelectorAll('.rec .prev.open').length, text: document.querySelector('.rec .prev.open')?.textContent ?? "" })`).then((s) => JSON.parse(s));
await evalJson(`(() => { const sel = document.querySelectorAll('select')[0]; sel.value = 'ai_step'; sel.dispatchEvent(new Event('change', { bubbles: true })); return true; })()`);
await sleep(400);
const po = await evalJson(`JSON.stringify({ open: document.querySelectorAll('.rec .prev.open').length, text: document.querySelector('.rec .prev.open')?.textContent ?? "", rows: document.querySelectorAll('.rec').length })`).then((s) => JSON.parse(s));
check("历史面板：筛选后展开态不错指到别的记录", ex.open === 1 && po.open <= 1 && (po.open === 0 || po.text === ex.text), `before=${ex.text.slice(0, 30)} after=${po.text.slice(0, 30)} rows=${po.rows}`);

check("历史面板：Esc 关闭", await escCloses("hist-title"));

// ---------- 4. 真机：连接 + 文件面板 + 预览弹窗（P99-1 / P101-1） ----------
let connected = await evalJson(`document.querySelector('.session-item .dot')?.classList.contains('ok') ?? false`);
if (!connected) {
  await evalJson(`(() => { const r = document.querySelector('.session-item'); if (!r) return false; r.dispatchEvent(new MouseEvent('dblclick', { bubbles: true })); return true; })()`);
  connected = await waitFor(`document.querySelector('.session-item .dot')?.classList.contains('ok') ?? false`, 60000);
}
check("会话已连接（真机）", connected === true, connected ? "" : "服务器不可达或未配置会话");

if (connected) {
  // 展开文件面板（默认折叠态点 .fold 是展开；已展开则不要再点，否则反而折叠）
  const wasCollapsed = await evalJson(`document.querySelector('.fs-panel')?.classList.contains('collapsed') ?? false`);
  if (wasCollapsed) await click(".fs-panel .fold");
  await waitFor(`document.querySelectorAll('.fs-row').length > 0`, 10000, 250);
  const cwd0 = await evalJson(`document.querySelector('.fs-cwd')?.textContent?.trim() ?? ''`);
  const rows = await evalJson(`document.querySelectorAll('.fs-row').length`);
  check("文件面板：展开后列出目录", rows > 0, `cwd=${cwd0} rows=${rows}`);

  // 目录行的标识是行内 .fs-icon.dir（行本身不带 dir 类）
  const dirRowJs = `[...document.querySelectorAll('.fs-row')].find((r) => r.querySelector('.fs-icon.dir'))`;
  const fileRowJs = `[...document.querySelectorAll('.fs-row')].find((r) => !r.querySelector('.fs-icon.dir'))`;

  // P99-1 观测证据：监听 .fs-cwd 的取值序列。旧实现 effect 被自身写的 cwd 二次触发，
  // 会用陈旧目标再刷一次 → 序列里出现「回到旧值再前进」；修复后一次导航只应有一次跃迁。
  // （注：包 window.__TAURI_INTERNALS__.invoke 计数不可行——Tauri 的 api 模块在加载时
  //   已捕获 invoke 引用，事后包装不在调用路径上。）
  await evalJson(`(() => { const el = document.querySelector('.fs-cwd'); window.__cwdHist = [el?.textContent?.trim() ?? '']; window.__cwdMo?.disconnect(); const anchor = el?.parentElement ?? el; if (anchor) { window.__cwdMo = new MutationObserver(() => { const v = document.querySelector('.fs-cwd')?.textContent?.trim() ?? ''; const h = window.__cwdHist; if (h[h.length - 1] !== v) h.push(v); }); window.__cwdMo.observe(anchor, { subtree: true, childList: true, characterData: true }); } return true; })()`);

  // P99-1：面板双击进目录后路径稳定（旧实现会因 effect 二次触发用陈旧目标回跳）
  const navigated = await evalJson(`(() => { const r = ${dirRowJs}; if (!r) return false; r.dispatchEvent(new MouseEvent('dblclick', { bubbles: true })); return true; })()`);
  await sleep(2500);
  const cwd1 = await evalJson(`document.querySelector('.fs-cwd')?.textContent?.trim() ?? ''`);
  await sleep(800);
  const cwd2 = await evalJson(`document.querySelector('.fs-cwd')?.textContent?.trim() ?? ''`);
  const hist = await evalJson(`JSON.stringify(window.__cwdHist ?? [])`).then((s) => JSON.parse(s));
  check("面板双击进目录后路径稳定（P99-1 无回跳）", navigated === true && !!cwd1 && cwd1 !== cwd0 && cwd1 === cwd2, `${cwd0} -> ${cwd1} -> ${cwd2}`);
  check("路径变化序列无回跳（一次导航只跃迁一次）", hist.length === 2 && hist[1] === cwd1, JSON.stringify(hist));

  // 预览弹窗 Esc
  const fileRow = await evalJson(`(() => { const r = ${fileRowJs}; if (!r) return false; r.dispatchEvent(new MouseEvent('dblclick', { bubbles: true })); return true; })()`);
  const pvOpen = await waitFor(`document.querySelector('[role=dialog][aria-labelledby=pv-title]') !== null`, 8000);
  if (fileRow === true && pvOpen) {
    check("文件预览：dialog 语义 + Esc 关闭", await escCloses("pv-title"));
  } else {
    check("文件预览：dialog 语义 + Esc 关闭", false, "未打开预览（该目录可能无可预览文件）");
  }
}

// ---------- 5. 全程零 JS 异常 ----------
check("全程无未捕获异常 / console.error", pageIssues.length === 0, pageIssues.slice(0, 3).join(" | "));

const failed = results.filter((r) => !r.ok).length;
console.log(`\n${results.length - failed}/${results.length} 项通过`);
ws.close();
process.exit(failed === 0 ? 0 : 1);
