// Helm 图标生成器:绘制 1024x1024 船舵(项目名 Helm = 船舵,"你掌舵,AI 执行")
// 零依赖,仅用 Node 内置 zlib 手写 PNG 编码。输出 RGBA PNG。
// 用法:node scripts/gen-icon.mjs [输出路径]
import zlib from "node:zlib";
import fs from "node:fs";

const SIZE = 1024;
const C = SIZE / 2;

// 品牌色(与前端 App.svelte 暗色主题一致)
const ACCENT = [0x4c, 0x8d, 0xff];
const LIGHT = [0xe9, 0xee, 0xf9];
const BG_TOP = [0x1e, 0x26, 0x37];
const BG_BOT = [0x0b, 0x0e, 0x14];

// ---------- 形状(内返回 true/false;靠 4x4 超采样抗锯齿) ----------
const inCircle = (x, y, r) => x * x + y * y <= r * r;
const inAnnulus = (x, y, rIn, rOut) => {
  const d2 = x * x + y * y;
  return d2 <= rOut * rOut && d2 >= rIn * rIn;
};
// 圆角矩形(以中心为原点)
const inRoundRect = (x, y, half, radius) => {
  const qx = Math.abs(x) - (half - radius);
  const qy = Math.abs(y) - (half - radius);
  const ax = Math.max(qx, 0);
  const ay = Math.max(qy, 0);
  if (qx <= 0 && qy <= 0) return true; // 内部区
  return ax * ax + ay * ay <= radius * radius;
};
// 线段(从原点沿角度 θ、半径 r0→r1,半宽 w)
const inSpoke = (x, y, theta, r0, r1, halfW) => {
  const cos = Math.cos(theta);
  const sin = Math.sin(theta);
  const t = Math.max(r0, Math.min(r1, x * cos + y * sin)); // 径向投影
  const dx = x - cos * t;
  const dy = y - sin * t;
  return dx * dx + dy * dy <= halfW * halfW;
};
const deg = (d) => (d * Math.PI) / 180;

// ---------- 舵轮几何 ----------
const RING_IN = 272;
const RING_OUT = 330;
const SPOKE_HALF_W = 19;
const SPOKE_R0 = 0;
const SPOKE_R1 = 395;
const KNOB_R = 428;
const KNOB_RAD = 34;
const HUB_LIGHT = 92;
const HUB_ACCENT = 48;
const SPOKES = 8; // 8 根辐条,每 45°

// accent 高亮弧(右上象限)
const inArc = (x, y) => {
  if (!inAnnulus(x, y, RING_IN, RING_OUT)) return false;
  let a = Math.atan2(x, -y); // 从正上方起算,弧度
  if (a < 0) a += Math.PI * 2;
  return a >= deg(28) && a <= deg(92);
};

// ---------- 逐采样求色 ----------
function sample(x, y) {
  if (!inRoundRect(x, y, C, 180)) return [0, 0, 0, 0];
  // 背景对角渐变
  const t = (x + y + SIZE) / (SIZE * 2);
  const bg = [
    BG_TOP[0] + (BG_BOT[0] - BG_TOP[0]) * t,
    BG_TOP[1] + (BG_BOT[1] - BG_TOP[1]) * t,
    BG_TOP[2] + (BG_BOT[2] - BG_TOP[2]) * t,
    255,
  ];
  // 舵轮(浅色)
  let wheel =
    inAnnulus(x, y, RING_IN, RING_OUT) ||
    inCircle(x, y, HUB_LIGHT);
  for (let i = 0; i < SPOKES; i++) {
    const th = deg(i * 45);
    if (inSpoke(x, y, th, SPOKE_R0, SPOKE_R1, SPOKE_HALF_W)) { wheel = true; break; }
    const kx = Math.cos(th) * KNOB_R;
    const ky = Math.sin(th) * KNOB_R;
    if (inCircle(x - kx, y - ky, KNOB_RAD)) { wheel = true; break; }
  }
  if (wheel) {
    // accent 细节:中心盘 + 外环高亮弧
    if (inCircle(x, y, HUB_ACCENT) || inArc(x, y)) return [...ACCENT, 255];
    return [...LIGHT, 255];
  }
  return bg;
}

// ---------- 渲染(4x4 超采样) ----------
const raw = Buffer.alloc(SIZE * (SIZE * 4 + 1));
const SS = 4;
const step = 1 / SS;
for (let py = 0; py < SIZE; py++) {
  const row = py * (SIZE * 4 + 1);
  raw[row] = 0; // filter: None
  for (let px = 0; px < SIZE; px++) {
    let r = 0, g = 0, b = 0, a = 0;
    for (let sy = 0; sy < SS; sy++) {
      for (let sx = 0; sx < SS; sx++) {
        const s = sample(px + (sx + 0.5) * step - C, py + (sy + 0.5) * step - C);
        r += s[0] * s[3];
        g += s[1] * s[3];
        b += s[2] * s[3];
        a += s[3];
      }
    }
    const o = row + 1 + px * 4;
    if (a === 0) {
      raw[o] = raw[o + 1] = raw[o + 2] = raw[o + 3] = 0;
    } else {
      raw[o] = Math.round(r / a);
      raw[o + 1] = Math.round(g / a);
      raw[o + 2] = Math.round(b / a);
      raw[o + 3] = Math.round(a / (SS * SS));
    }
  }
}

// ---------- PNG 编码 ----------
const CRC_TABLE = (() => {
  const t = new Uint32Array(256);
  for (let n = 0; n < 256; n++) {
    let c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    t[n] = c >>> 0;
  }
  return t;
})();
const crc32 = (buf) => {
  let c = 0xffffffff;
  for (const b of buf) c = CRC_TABLE[(c ^ b) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
};
const chunk = (type, data) => {
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length);
  const body = Buffer.concat([Buffer.from(type, "ascii"), data]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(body));
  return Buffer.concat([len, body, crc]);
};

const ihdr = Buffer.alloc(13);
ihdr.writeUInt32BE(SIZE, 0);
ihdr.writeUInt32BE(SIZE, 4);
ihdr[8] = 8; // bit depth
ihdr[9] = 6; // RGBA
const png = Buffer.concat([
  Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
  chunk("IHDR", ihdr),
  chunk("IDAT", zlib.deflateSync(raw, { level: 9 })),
  chunk("IEND", Buffer.alloc(0)),
]);

const out = process.argv[2] || "assets/icon-1024.png";
fs.mkdirSync(out.slice(0, out.lastIndexOf("/")), { recursive: true });
fs.writeFileSync(out, png);
console.log(`written ${out} (${png.length} bytes)`);
