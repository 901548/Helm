// 路径纯函数(自 FileBrowser 抽出,P44):跨平台(linux / windows)拼接、
// cd 目标解析、shell 引号转义、大小格式化。配 Vitest 测试。

/// 按平台拼接路径(sep: linux "/" / windows "\")
export function joinPath(base: string, name: string, sep: "/" | "\\"): string {
  if (!base) return name;
  return base.endsWith("/") || base.endsWith("\\") ? base + name : base + sep + name;
}

/// 解析终端 cd 目标为规范路径:
/// - `~` / `~/x` 保留波浪号(后端负责展开)
/// - 相对路径基于 cwd 拼绝对
/// - 剥尾斜杠(根与 `~` 除外);识别 windows 驱动盘绝对路径
export function resolvePath(raw: string, cwd: string, sep: "/" | "\\"): string {
  let p = raw.trim();
  if (!p) return "~";
  if (p === "~") return "~";
  const isAbs = p.startsWith("/") || /^[a-zA-Z]:[\\/]/.test(p);
  if (!isAbs) {
    if (p.startsWith("~/") || p.startsWith("~\\")) {
      // 保留波浪号首段(后端展开),仍剥尾斜杠
      p = "~" + p.slice(1);
    } else {
      p = joinPath(cwd, p, sep);
    }
  }
  while (p.length > 1 && (p.endsWith("/") || p.endsWith("\\"))) p = p.slice(0, -1);
  return p;
}

/// windows 盘符路径上一级(`C:\a\b` → `C:\a`;盘符根 `C:\` → `\`;无分隔符 → null 不动)
export function parentPathWindows(cwd: string): string | null {
  const idx = Math.max(cwd.lastIndexOf("\\"), cwd.lastIndexOf("/"));
  if (idx < 0) return null;
  const parent = cwd.slice(0, idx);
  if (parent && !/^[a-zA-Z]:$/.test(parent)) return parent;
  return "\\";
}

/// bash 单引号安全包裹(`'` → `'\''`)
export function shq(p: string): string {
  return "'" + p.replace(/'/g, "'\\''") + "'";
}

/// 字节数人类可读
export function fmtSize(n: number): string {
  if (n >= 1024 * 1024 * 1024) return (n / 1024 / 1024 / 1024).toFixed(1) + "G";
  if (n >= 1024 * 1024) return (n / 1024 / 1024).toFixed(1) + "M";
  if (n >= 1024) return (n / 1024).toFixed(0) + "K";
  return n + "B";
}
