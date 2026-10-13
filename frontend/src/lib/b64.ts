// P126：文件预览弹窗的 base64 编码逻辑抽为纯函数（供单测）
//
// 背景：FilePreviewModal 保存时要把文本编码为 base64 交给 fsUpload。原实现在
// FileBrowser 里用 `for (const b of bytes) bin += String.fromCharCode(b)` —— 逐字节
// 字符串拼接，大文件（60KB 预览上限）会退化成 O(n²) 且可能触发参数长度限制。
// 抽出来用分块 + spread，并补单测锁住「多字节字符不乱码」这个关键点。

/** 分块大小：避免 String.fromCharCode 参数过多（V8 上限约 65535） */
const CHUNK = 0x8000;

/** 字节数组 → base64（ZMODEM 分块上传、文件上传分块共用） */
export function bytesToBase64(bytes: Uint8Array): string {
  let bin = "";
  for (let i = 0; i < bytes.length; i += CHUNK) {
    bin += String.fromCharCode(...bytes.subarray(i, i + CHUNK));
  }
  return btoa(bin);
}

/**
 * 文本 → base64（UTF-8）。
 * 多字节字符（中文/emoji）必须先经 TextEncoder 转 UTF-8 字节，
 * 直接对 JS 字符串 btoa 会在 charCode > 255 时抛 InvalidCharacterError。
 */
export function textToBase64(text: string): string {
  return bytesToBase64(new TextEncoder().encode(text));
}

/** base64 → UTF-8 文本（下载回读用） */
export function base64ToText(b64: string): string {
  const bin = atob(b64);
  const bytes = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
  return new TextDecoder().decode(bytes);
}

/** base64 → Uint8Array（下载成 Blob 用，避免二次解码） */
export function base64ToBytes(b64: string): Uint8Array {
  const bin = atob(b64);
  const bytes = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
  return bytes;
}
