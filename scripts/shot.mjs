// CDP 截图工具:node scripts/shot.mjs [端口] [输出路径]
// 连接 WebView2 调试端口截取页面 PNG
import fs from "node:fs";

const port = process.argv[2] || "9229";
const out = process.argv[3] || "shot.png";

const list = await (await fetch(`http://127.0.0.1:${port}/json`)).json();
const page = list.find((p) => p.type === "page" && p.webSocketDebuggerUrl);
if (!page) {
  console.error("no debuggable page found:", list.map((p) => p.type));
  process.exit(1);
}

const ws = new WebSocket(page.webSocketDebuggerUrl);
let id = 0;
const pending = new Map();
const send = (method, params = {}) =>
  new Promise((res, rej) => {
    const mid = ++id;
    pending.set(mid, { res, rej });
    ws.send(JSON.stringify({ id: mid, method, params }));
  });

ws.onmessage = (ev) => {
  const msg = JSON.parse(ev.data);
  if (msg.id && pending.has(msg.id)) {
    const { res, rej } = pending.get(msg.id);
    pending.delete(msg.id);
    msg.error ? rej(new Error(msg.error.message)) : res(msg.result);
  }
};
await new Promise((r) => (ws.onopen = r));

await send("Page.bringToFront").catch(() => {});
const shot = await send("Page.captureScreenshot", { format: "png" });
fs.writeFileSync(out, Buffer.from(shot.data, "base64"));
console.log(`saved ${out} (${Math.round(shot.data.length / 1024)}KB b64)`);
ws.close();
