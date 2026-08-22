import { describe, expect, it } from "vitest";
import { fmtSize, joinPath, parentPathWindows, resolvePath, shq } from "./paths";

describe("joinPath", () => {
  it("linux 拼接", () => {
    expect(joinPath("/opt", "logs", "/")).toBe("/opt/logs");
  });
  it("windows 拼接", () => {
    expect(joinPath("C:\\Users", "me", "\\")).toBe("C:\\Users\\me");
  });
  it("base 已带尾分隔符不重复", () => {
    expect(joinPath("/opt/", "logs", "/")).toBe("/opt/logs");
    expect(joinPath("C:\\", "temp", "\\")).toBe("C:\\temp");
  });
  it("空 base 直接返回 name", () => {
    expect(joinPath("", "tmp", "/")).toBe("tmp");
  });
});

describe("resolvePath", () => {
  it("空与 ~ 保留波浪号(后端展开)", () => {
    expect(resolvePath("", "/root", "/")).toBe("~");
    expect(resolvePath("~", "/root", "/")).toBe("~");
  });
  it("~/x 归一化为 ~ 风格", () => {
    expect(resolvePath("~/data/", "/root", "/")).toBe("~/data");
  });
  it("相对路径基于 cwd 拼绝对", () => {
    expect(resolvePath("logs", "/var", "/")).toBe("/var/logs");
  });
  it("绝对路径原样 + 剥尾斜杠(根除外)", () => {
    expect(resolvePath("/usr/local/", "/root", "/")).toBe("/usr/local");
    expect(resolvePath("/", "/root", "/")).toBe("/");
  });
  it("windows 驱动盘绝对路径", () => {
    expect(resolvePath("D:\\Tools\\", "C:\\Users", "\\")).toBe("D:\\Tools");
    expect(resolvePath("D:/Tools", "C:\\Users", "\\")).toBe("D:/Tools");
  });
});

describe("parentPathWindows", () => {
  it("普通路径上一级", () => {
    expect(parentPathWindows("C:\\a\\b")).toBe("C:\\a");
  });
  it("盘符根退到 \\", () => {
    expect(parentPathWindows("C:\\")).toBe("\\");
  });
  it("无分隔符返回 null(不动)", () => {
    expect(parentPathWindows("C:")).toBeNull();
  });
});

describe("shq", () => {
  it("普通路径单引号包裹", () => {
    expect(shq("/usr/local")).toBe("'/usr/local'");
  });
  it("含空格原样安全", () => {
    expect(shq("/opt/my dir")).toBe("'/opt/my dir'");
  });
  it("含单引号转义为 '\\''", () => {
    expect(shq("/a'b")).toBe("'/a'\\''b'");
  });
});

describe("fmtSize", () => {
  it("各级单位", () => {
    expect(fmtSize(500)).toBe("500B");
    expect(fmtSize(2048)).toBe("2K");
    expect(fmtSize(3 * 1024 * 1024)).toBe("3.0M");
    expect(fmtSize(2 * 1024 * 1024 * 1024)).toBe("2.0G");
  });
});
