import { describe, it, expect } from "vitest";
import {
  defaultAiSession,
  getAi,
  setAi,
  updateAi,
  renameAiSession,
  dropAiSession,
  type AiSessionMap,
} from "./aiSession";
import type { AiQaCard } from "./types";

const card = (id: number, text: string): AiQaCard => ({ id, kind: "qa", text, done: true });

describe("defaultAiSession / getAi", () => {
  it("缺省态工厂每次返回独立对象（勿共享引用）", () => {
    const a = defaultAiSession();
    const b = defaultAiSession();
    expect(a).not.toBe(b);
    expect(a).toEqual({
      busy: false,
      state: "idle",
      mode: "qa",
      cards: [],
      taskText: "",
      summary: null,
      streamOpen: false,
      thinking: "",
    });
    expect(a.cards).not.toBe(b.cards);
  });

  it("getAi 对缺失会话返回缺省态、对已有会话返回原引用", () => {
    const rec: AiSessionMap = {};
    expect(getAi(rec, "s1")).toEqual(defaultAiSession());

    setAi(rec, "s1", { busy: true });
    const got = getAi(rec, "s1");
    expect(got.busy).toBe(true);
    expect(got).toBe(rec["s1"]);
  });
});

describe("setAi", () => {
  it("部分补丁只改给定字段，其余保持", () => {
    const rec: AiSessionMap = {};
    setAi(rec, "s1", { mode: "agent", taskText: "查磁盘" });
    setAi(rec, "s1", { busy: true });
    expect(rec["s1"].mode).toBe("agent");
    expect(rec["s1"].taskText).toBe("查磁盘");
    expect(rec["s1"].busy).toBe(true);
    expect(rec["s1"].state).toBe("idle");
  });

  it("写入保持条目对象引用（Svelte 5 细粒度响应式的关键设计）", () => {
    const rec: AiSessionMap = {};
    setAi(rec, "s1", { mode: "agent" });
    const ref = rec["s1"];
    setAi(rec, "s1", { busy: true });
    setAi(rec, "s1", { state: "planning" });
    expect(rec["s1"]).toBe(ref);
  });
});

describe("updateAi", () => {
  it("fn 收到当前态：cards 连续追加累积", () => {
    const rec: AiSessionMap = {};
    updateAi(rec, "s1", (s) => ({ cards: [...s.cards, card(1, "a")] }));
    updateAi(rec, "s1", (s) => ({ cards: [...s.cards, card(2, "b")] }));
    expect(rec["s1"].cards.map((c) => c.id)).toEqual([1, 2]);
  });

  it("缺失条目时以缺省态起步", () => {
    const rec: AiSessionMap = {};
    updateAi(rec, "s1", (s) => ({ thinking: s.thinking + "推理" }));
    expect(rec["s1"].thinking).toBe("推理");
    expect(rec["s1"].cards).toEqual([]);
  });

  it("写入同样保持条目对象引用", () => {
    const rec: AiSessionMap = {};
    updateAi(rec, "s1", (s) => ({ summary: { text: "完成", ok: true } }));
    const ref = rec["s1"];
    updateAi(rec, "s1", (s) => ({ cards: [...s.cards, card(1, "x")] }));
    expect(rec["s1"]).toBe(ref);
  });
});

describe("renameAiSession", () => {
  it("只迁移 mode/streamOpen，运行态字段复位；源条目删除", () => {
    const rec: AiSessionMap = {};
    setAi(rec, "old", {
      mode: "agent",
      streamOpen: true,
      busy: true,
      state: "executing",
      taskText: "任务",
      summary: { text: "进行中", ok: true },
      thinking: "推理",
    });
    updateAi(rec, "old", (s) => ({ cards: [...s.cards, card(1, "x")] }));

    renameAiSession(rec, "old", "new");
    expect("old" in rec).toBe(false);
    expect(rec["new"].mode).toBe("agent");
    expect(rec["new"].streamOpen).toBe(true);
    // 卡片/任务/摘要/思考不迁移——由 $effect(activeTab) 用后端 conv 重建
    expect(rec["new"].cards).toEqual([]);
    expect(rec["new"].taskText).toBe("");
    expect(rec["new"].summary).toBeNull();
    expect(rec["new"].thinking).toBe("");
    expect(rec["new"].busy).toBe(false);
    expect(rec["new"].state).toBe("idle");
  });

  it("from === to 为 no-op（保留全部运行态）", () => {
    const rec: AiSessionMap = {};
    setAi(rec, "s1", { busy: true, mode: "agent" });
    updateAi(rec, "s1", (s) => ({ cards: [...s.cards, card(1, "x")] }));
    renameAiSession(rec, "s1", "s1");
    expect(rec["s1"].busy).toBe(true);
    expect(rec["s1"].cards).toHaveLength(1);
  });

  it("源条目不存在时不新建目标条目", () => {
    const rec: AiSessionMap = {};
    renameAiSession(rec, "ghost", "new");
    expect("new" in rec).toBe(false);
    expect(Object.keys(rec)).toEqual([]);
  });

  it("目标已有条目时保留其运行态，仅覆盖 mode/streamOpen", () => {
    const rec: AiSessionMap = {};
    setAi(rec, "old", { mode: "agent", streamOpen: true });
    setAi(rec, "new", { mode: "qa", streamOpen: false, taskText: "已有任务" });
    updateAi(rec, "new", (s) => ({ cards: [...s.cards, card(9, "keep")] }));

    renameAiSession(rec, "old", "new");
    expect(rec["new"].mode).toBe("agent");
    expect(rec["new"].streamOpen).toBe(true);
    expect(rec["new"].taskText).toBe("已有任务");
    expect(rec["new"].cards.map((c) => c.id)).toEqual([9]);
  });
});

describe("dropAiSession / 会话隔离", () => {
  it("删除指定条目；不存在的名字不抛错", () => {
    const rec: AiSessionMap = {};
    setAi(rec, "s1", { busy: true });
    dropAiSession(rec, "s1");
    dropAiSession(rec, "ghost");
    expect(Object.keys(rec)).toEqual([]);
  });

  it("多会话互不串扰（写 A 不影响 B）", () => {
    const rec: AiSessionMap = {};
    setAi(rec, "a", { mode: "agent", busy: true });
    setAi(rec, "b", { mode: "qa" });
    updateAi(rec, "a", (s) => ({ cards: [...s.cards, card(1, "a1")] }));
    expect(rec["b"].mode).toBe("qa");
    expect(rec["b"].busy).toBe(false);
    expect(rec["b"].cards).toEqual([]);
  });
});
