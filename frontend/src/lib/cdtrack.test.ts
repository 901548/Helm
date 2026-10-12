import { describe, it, expect } from "vitest";
import { initialCdState, trackCdFallback, MAX_ECHO_TAIL } from "./cdtrack";

const T = 1_000_000;

/** 便捷：从初始态消费一段文本 */
function run(text: string, state = initialCdState(), now = T) {
  return trackCdFallback(state, text, now);
}

describe("trackCdFallback 登记与确认", () => {
  it("`]# cd /etc` + 紧随提示符 → 触发（提示符无尾换行也能确认）", () => {
    // 关键回归：成功提示符行不带 \n，必须把未成行尾段也投入扫描
    const r = run("[root@h ~]# cd /etc\n[root@h etc]# ");
    expect(r.fires).toEqual(["/etc"]);
    expect(r.state.pending).toBeNull();
  });

  it("带引号的路径去掉引号（面板双击生成的 `cd '/a b'`）", () => {
    const r = run("[root@h ~]# cd '/usr/local/hadoop'\n[root@h hadoop]# ");
    expect(r.fires).toEqual(["/usr/local/hadoop"]);
  });

  it("`$ cd` 无参数 → 目标为 ~", () => {
    const r = run("[u@h x]$ cd\n[u@h ~]$ ");
    expect(r.fires).toEqual(["~"]);
  });

  it("`cd -` 不触发（目标未知）", () => {
    const r = run("[u@h x]$ cd -\n[u@h y]$ ");
    expect(r.fires).toEqual([]);
    expect(r.state.pending).toBeNull();
  });

  it("跨块：登记在上一块、确认在下一块", () => {
    const a = run("[root@h ~]# cd /var");
    expect(a.fires).toEqual([]);
    expect(a.state.pending?.target).toBe("/var");
    const b = trackCdFallback(a.state, "\n[root@h var]# ", T + 100);
    expect(b.fires).toEqual(["/var"]);
  });
});

describe("trackCdFallback 反例（不得误触发）", () => {
  it("失败行作废：`No such file` 后不触发", () => {
    const r = run("[root@h ~]# cd /nope\n-bash: cd: /nope: No such file or directory\n[root@h ~]# ");
    expect(r.fires).toEqual([]);
  });

  it("中文错误行也作废（靠 `: cd(` 前缀命中）", () => {
    const r = run("[root@h ~]# cd /nope\n-bash: cd: /nope: 没有那个文件或目录\n[root@h ~]# ");
    expect(r.fires).toEqual([]);
  });

  it("中间有其他输出即作废（防 `echo 'cd /tmp'` 误触发）", () => {
    // 注意：这里"登记"行本身是提示符锚定的复合行，故 echo 的输出不会登记；
    // 但即便登记了，随后的非提示符输出也应作废
    const r = run("[root@h ~]# cd /tmp\nsome program output\n[root@h ~]# ");
    expect(r.fires).toEqual([]);
  });

  it("普通命令输出不登记（无 `]# cd` 形态）", () => {
    const r = run("total 12\ndrwxr-xr-x 2 root root 4096 a\n");
    expect(r.fires).toEqual([]);
    expect(r.state.pending).toBeNull();
  });

  it("完整提示符形态超时后不确认（窗口不被重复扫描续期）", () => {
    const a = run("[root@h ~]# cd /opt");
    expect(a.state.pending?.target).toBe("/opt");
    // 超时 5s 后才出现 user@host 形态提示符 → 不认。
    // 关键：登记行还在 buf 里会被重新扫描，若每次刷新 t 则窗口永不过期（原实现缺陷）
    const late = trackCdFallback(a.state, "\nroot@h:/opt# ", T + 5000);
    expect(late.fires).toEqual([]);
  });

  it("完整提示符形态在窗口内则确认", () => {
    const a = run("[root@h ~]# cd /opt");
    const soon = trackCdFallback(a.state, "\nroot@h:/opt# ", T + 500);
    expect(soon.fires).toEqual(["/opt"]);
  });
});

describe("trackCdFallback 缓冲管理", () => {
  it("未成行尾段留在缓冲里（供下次拼接）", () => {
    const r = run("partial-line-without-newline");
    expect(r.state.buf).toBe("partial-line-without-newline");
  });

  it("以换行结尾时缓冲清空", () => {
    const r = run("line one\nline two\n");
    expect(r.state.buf).toBe("");
  });

  it("尾缓冲有上限，防长时间刷屏撑大内存", () => {
    const huge = "x".repeat(MAX_ECHO_TAIL + 5000);
    const r = run(huge);
    expect(r.state.buf.length).toBeLessThanOrEqual(MAX_ECHO_TAIL);
  });

  it("跨块拼接后能识别被切断的登记行", () => {
    // 半行立即登记（与组件原行为一致：未成行尾段也投入扫描，否则提示符会滞留不确认）
    const a = run("[root@h ~]# cd /va");
    expect(a.state.pending?.target).toBe("/va");
    expect(a.state.buf).toBe("[root@h ~]# cd /va");
    // 补齐后重新扫描到完整行 `/var`（目标以最终拼成的整行为准），随后提示符确认
    const b = trackCdFallback(a.state, "r\n[root@h var]# ", T + 50);
    expect(b.fires).toEqual(["/var"]);
  });
});
