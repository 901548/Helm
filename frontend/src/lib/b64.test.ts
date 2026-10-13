import { describe, it, expect } from "vitest";
import { textToBase64, base64ToText, base64ToBytes, bytesToBase64 } from "./b64";

describe("textToBase64", () => {
  it("ASCII 往返一致", () => {
    const s = "hello world\nline2";
    expect(base64ToText(textToBase64(s))).toBe(s);
  });

  it("中文/emoji 不乱码（charCode>255 不能直接 btoa）", () => {
    // 关键回归：直接 btoa("中文") 会抛 InvalidCharacterError
    const s = "中文测试 · 你好世界 🚀 舵";
    const b64 = textToBase64(s);
    expect(base64ToText(b64)).toBe(s);
    // 确认真的按 UTF-8 编码（"中" = E4 B8 AD）
    expect(base64ToBytes(b64).slice(0, 3)).toEqual(new Uint8Array([0xe4, 0xb8, 0xad]));
  });

  it("空串得到空 base64", () => {
    expect(textToBase64("")).toBe("");
    expect(base64ToText("")).toBe("");
  });

  it("跨分块边界的长文本（> 0x8000 字节）仍正确", () => {
    // 每字符 3 字节 → 20000 字符 ≈ 60000 字节，跨越 0x8000 分块边界
    const s = "中".repeat(20000);
    const b64 = textToBase64(s);
    expect(base64ToBytes(b64).length).toBe(60000);
    expect(base64ToText(b64)).toBe(s);
  });

  it("预览上限量级（60KB 中文）不抛异常", () => {
    const s = "运维日志行\n".repeat(6000); // 约 60KB（中文 3 字节/字）
    expect(() => textToBase64(s)).not.toThrow();
    expect(base64ToText(textToBase64(s))).toBe(s);
  });

  it("换行与制表符原样保留（终端日志/配置常见）", () => {
    const s = "a\tb\r\nc\nd";
    expect(base64ToText(textToBase64(s))).toBe(s);
  });

  it("base64ToBytes 与 base64ToText 对同一输入一致", () => {
    const s = "混合 mixed 内容 123";
    const b64 = textToBase64(s);
    expect(new TextDecoder().decode(base64ToBytes(b64))).toBe(s);
  });
});

describe("bytesToBase64（文件上传/ZMODEM 分块共用）", () => {
  it("二进制字节往返一致（含 0x00/0xFF 等非文本字节）", () => {
    const bytes = new Uint8Array([0x00, 0x01, 0x7f, 0x80, 0xfe, 0xff, 0x00]);
    expect(base64ToBytes(bytesToBase64(bytes))).toEqual(bytes);
  });

  it("空数组得到空 base64", () => {
    expect(bytesToBase64(new Uint8Array(0))).toBe("");
  });

  it("跨 0x8000 分块边界的大数组仍正确（64KB 上传分块量级）", () => {
    const bytes = new Uint8Array(70000);
    for (let i = 0; i < bytes.length; i++) bytes[i] = i & 0xff;
    const back = base64ToBytes(bytesToBase64(bytes));
    expect(back.length).toBe(bytes.length);
    expect(back).toEqual(bytes);
  });

  it("textToBase64 与 bytesToBase64(TextEncoder) 等价", () => {
    const s = "一致 性 check 🚀";
    expect(textToBase64(s)).toBe(bytesToBase64(new TextEncoder().encode(s)));
  });
});
