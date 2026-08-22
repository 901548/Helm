// CDP: 在页面里执行 JS 并打印结果 node scripts/eval.mjs <端口> <表达式>
const port = process.argv[2] || "9229";
const expr = process.argv[3];

const list = await (await fetch(`http://127.0.0.1:${port}/json`)).json();
const page = list.find((p) => p.type === "page" && p.webSocketDebuggerUrl);
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
const r = await send("Runtime.evaluate", { expression: expr, returnByValue: true, awaitPromise: true });
console.log(r.exceptionDetails ? "EXCEPTION: " + JSON.stringify(r.exceptionDetails.exception?.description || r.exceptionDetails.text) : JSON.stringify(r.result.value, null, 1));
ws.close();
process.exit(0);
