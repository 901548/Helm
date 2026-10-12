import { describe, it, expect } from "vitest";
import { convToCard, convToCards, convTaskText, type ConvEntry } from "./conv";

describe("convToCard", () => {
  it("qa 条目映射为已完成问答卡（字段是 a 不是 text）", () => {
    const c = convToCard({ kind: "qa", q: "问题", a: "回答" }, 7);
    expect(c).toEqual({ id: 7, kind: "qa", text: "回答", done: true });
  });

  it("plan 条目带 need_confirm → needConfirm（后端 snake_case 字段）", () => {
    const c = convToCard(
      { kind: "plan", commands: [{ command: "ls" }], need_confirm: true },
      1,
    );
    expect(c).toMatchObject({ kind: "plan", status: "plan", needConfirm: true });
    if (c.kind === "plan") expect(c.commands).toHaveLength(1);
  });

  it("plan 缺 commands 时退化为空数组（不产生 undefined 渲染）", () => {
    const c = convToCard({ kind: "plan" }, 1);
    if (c.kind === "plan") expect(c.commands).toEqual([]);
  });

  it("goal 条目保留 ok/failed，未知状态归 active", () => {
    expect(convToCard({ kind: "goal", status: "ok", index: 2, total: 3, title: "T" }, 1)).toMatchObject({
      kind: "goal",
      status: "ok",
      index: 2,
      total: 3,
      title: "T",
    });
    expect(convToCard({ kind: "goal", status: "failed" }, 1)).toMatchObject({ status: "failed" });
    // 后端可能给 running/缺省 → 前端一律显示 active
    expect(convToCard({ kind: "goal", status: "running" }, 1)).toMatchObject({ status: "active" });
    expect(convToCard({ kind: "goal" }, 1)).toMatchObject({ status: "active" });
  });

  it("step 以 success 判 ok/fail，缺 output 时为 undefined", () => {
    expect(convToCard({ kind: "step", command: "df -h", success: true }, 3)).toEqual({
      id: 3,
      kind: "step",
      command: "df -h",
      status: "ok",
      message: "",
      output: undefined,
    });
    expect(convToCard({ kind: "step", command: "x", success: false }, 4)).toMatchObject({
      status: "fail",
    });
  });

  it("未知 kind 兜底为 step（后端新增变体时前端不崩）", () => {
    expect(convToCard({ kind: "future_thing", command: "c" }, 1)).toMatchObject({
      kind: "step",
      command: "c",
    });
    expect(convToCard({}, 1)).toMatchObject({ kind: "step", status: "fail" });
  });

  it("null 字段不产生字符串 'undefined'/'null'", () => {
    const c = convToCard({ kind: "qa", a: null }, 1);
    if (c.kind === "qa") expect(c.text).toBe("");
    const s = convToCard({ kind: "step", command: null, message: null }, 1);
    if (s.kind === "step") {
      expect(s.command).toBe("");
      expect(s.message).toBe("");
    }
  });
});

describe("convTaskText / convToCards", () => {
  it("task 条目的 task 字段作为任务文本，缺失时为空串", () => {
    expect(convTaskText([{ kind: "task", task: "查磁盘" }, { kind: "qa" }])).toBe("查磁盘");
    expect(convTaskText([{ kind: "qa" }])).toBe("");
    expect(convTaskText([])).toBe("");
  });

  it("convToCards 跳过 task 条目，且 id 由取号器分配（互不重复）", () => {
    const conv: ConvEntry[] = [
      { kind: "task", task: "T" },
      { kind: "goal", status: "ok", index: 1, total: 1, title: "G" },
      { kind: "step", command: "a", success: true },
      { kind: "qa", a: "答" },
    ];
    let n = 0;
    const cards = convToCards(conv, () => ++n);
    expect(cards).toHaveLength(3);
    expect(cards.map((c) => c.kind)).toEqual(["goal", "step", "qa"]);
    expect(cards.map((c) => c.id)).toEqual([1, 2, 3]);
    // 顺序即后端给的新→旧顺序，不做重排（Goal 分组依赖相邻关系）
    expect(n).toBe(3);
  });

  it("空记录得到空卡片列表", () => {
    expect(convToCards([], () => 1)).toEqual([]);
  });
});
