import { describe, expect, it } from "vitest";
import { normalizePwd, parseOscPwd, stripAnsi } from "./osc";

const BEL = "\x07";
const osc = (pwd: string) => `\x1b]7;helm:${pwd}${BEL}`;

describe("parseOscPwd", () => {
  it("完整标记提取 PWD", () => {
    const r = parseOscPwd("", `[root@master ~]# ${osc("/root")}`);
    expect(r.pwd).toBe("/root");
    expect(r.rest).toBe("");
  });

  it("同块多个标记取最后一个(多次提示符重画)", () => {
    const r = parseOscPwd("", `${osc("/root")}${osc("/etc")}${osc("/opt")}`);
    expect(r.pwd).toBe("/opt");
  });

  it("跨 TCP 分包:前半片段留待下块拼接", () => {
    const r1 = parseOscPwd("", "output...\x1b]7;helm:/usr/loc");
    expect(r1.pwd).toBeNull();
    expect(r1.rest).toBe("\x1b]7;helm:/usr/loc");
    const r2 = parseOscPwd(r1.rest, `al${BEL}\r\n[root@master local]# `);
    expect(r2.pwd).toBe("/usr/local");
    expect(r2.rest).toBe("");
  });

  it("非 helm 前缀的 OSC7 不误取", () => {
    const r = parseOscPwd("", `\x1b]7;file://host/root${BEL}`);
    expect(r.pwd).toBeNull();
  });

  it("无标记时普通输出不残留缓冲", () => {
    const r = parseOscPwd("", "just some plain output\r\n");
    expect(r.pwd).toBeNull();
    expect(r.rest).toBe("");
  });
});

describe("stripAnsi", () => {
  it("剥 CSI 颜色码", () => {
    expect(stripAnsi("\x1b[1;31m红\x1b[0m")).toBe("红");
  });

  it("剥完整 OSC 序列(含 OSC7 标记)", () => {
    expect(stripAnsi(`[root@master ~]# ${osc("/root")} `)).toBe("[root@master ~]#  ");
  });

  it("光标定位/擦除码", () => {
    expect(stripAnsi("\x1b[2K\x1b[1Gdone")).toBe("done");
  });
});

describe("normalizePwd", () => {
  it("剥尾斜杠", () => {
    expect(normalizePwd("/usr/local/")).toBe("/usr/local");
  });
  it("根保留单斜杠", () => {
    expect(normalizePwd("/")).toBe("/");
  });
});
