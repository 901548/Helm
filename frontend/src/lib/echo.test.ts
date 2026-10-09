import { describe, expect, it } from "vitest";
import { replayEcho, type EchoEntry } from "./echo";

const e = (seq: number, name: string, text = `t${seq}`): EchoEntry => ({ seq, name, text });

describe("replayEcho", () => {
  it("顺序投递并推进各会话游标", () => {
    const { writes, next } = replayEcho([e(1, "A"), e(2, "A")], new Map(), () => true);
    expect(writes.map((w) => w.text)).toEqual(["t1", "t2"]);
    expect(next.get("A")).toBe(2);
  });

  it("终端未建的条目保留待写：低 seq 不被同批更高 seq 越过（P99-2 回归点）", () => {
    // A 的终端还没建，B 的已建：旧的全局游标会被 seq=2 推过去，导致 seq=1 永久丢失
    const { writes, next } = replayEcho([e(1, "A"), e(2, "B")], new Map(), (n) => n === "B");
    expect(writes.map((w) => w.text)).toEqual(["t2"]); // 只写了 B
    expect(next.get("A")).toBeUndefined(); // A 的游标不动 → 保留待写
    expect(next.get("B")).toBe(2);
  });

  it("终端建立后补写滞留条目（termVersion 重放路径）", () => {
    const first = replayEcho([e(1, "A"), e(2, "B")], new Map(), (n) => n === "B");
    const second = replayEcho([e(1, "A"), e(2, "B")], first.next, () => true);
    expect(second.writes.map((w) => w.text)).toEqual(["t1"]); // 只补写尚未落地的 A
    expect(second.next.get("A")).toBe(1);
  });

  it("已写过的条目不重复写（幂等）", () => {
    const cur = new Map([["A", 5]]);
    const { writes, next } = replayEcho([e(3, "A"), e(5, "A"), e(6, "A")], cur, () => true);
    expect(writes.map((w) => w.text)).toEqual(["t6"]);
    expect(next.get("A")).toBe(6);
  });

  it("一个会话永久无终端也不阻塞其它会话", () => {
    const entries = [e(1, "dead"), e(2, "A"), e(3, "dead"), e(4, "A")];
    const { writes, next } = replayEcho(entries, new Map(), (n) => n === "A");
    expect(writes.map((w) => w.text)).toEqual(["t2", "t4"]);
    expect(next.get("dead")).toBeUndefined();
    expect(next.get("A")).toBe(4);
  });

  it("不修改传入的游标表（调用方自行替换）", () => {
    const cur = new Map([["A", 1]]);
    replayEcho([e(2, "A")], cur, () => true);
    expect(cur.get("A")).toBe(1);
  });
});
