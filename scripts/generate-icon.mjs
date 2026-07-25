// Generates the LocalConvert source icon: a rounded square with a
// convert-arrow glyph, drawn straight into an RGBA buffer and zlib-deflated
// into a PNG. Written here rather than committed as a binary blob so the icon
// is reproducible from source.
import { deflateSync } from "node:zlib";
import { writeFileSync } from "node:fs";
import { crc32 } from "node:zlib";

const S = 1024;
const px = Buffer.alloc(S * S * 4);

const set = (x, y, r, g, b, a) => {
  const i = (y * S + x) * 4;
  // simple source-over
  const sa = a / 255;
  px[i] = Math.round(px[i] * (1 - sa) + r * sa);
  px[i + 1] = Math.round(px[i + 1] * (1 - sa) + g * sa);
  px[i + 2] = Math.round(px[i + 2] * (1 - sa) + b * sa);
  px[i + 3] = Math.min(255, Math.round(px[i + 3] + a * (1 - px[i + 3] / 255)));
};

// Rounded-square background with a vertical gradient.
const R = 200;
const inset = 40;
const cover = (x, y) => {
  const lo = inset, hi = S - inset;
  if (x < lo || x > hi || y < lo || y > hi) return 0;
  const cx = Math.min(Math.max(x, lo + R), hi - R);
  const cy = Math.min(Math.max(y, lo + R), hi - R);
  const d = Math.hypot(x - cx, y - cy);
  return Math.max(0, Math.min(1, R - d + 0.5));
};

for (let y = 0; y < S; y++) {
  const t = y / S;
  const r = Math.round(37 + t * 20);
  const g = Math.round(99 + t * 40);
  const b = Math.round(235 - t * 55);
  for (let x = 0; x < S; x++) {
    const a = cover(x, y);
    if (a > 0) set(x, y, r, g, b, Math.round(a * 255));
  }
}

// Two opposing arrows (convert), drawn as thick strokes + triangular heads.
const white = (x, y, a = 255) => set(Math.round(x), Math.round(y), 255, 255, 255, a);
const stroke = (x0, y0, x1, y1, w) => {
  const steps = Math.ceil(Math.hypot(x1 - x0, y1 - y0) * 2);
  for (let s = 0; s <= steps; s++) {
    const x = x0 + ((x1 - x0) * s) / steps;
    const y = y0 + ((y1 - y0) * s) / steps;
    for (let dy = -w; dy <= w; dy++)
      for (let dx = -w; dx <= w; dx++) {
        const d = Math.hypot(dx, dy);
        if (d <= w) white(x + dx, y + dy, d > w - 1.5 ? 140 : 255);
      }
  }
};
// Triangle that tapers to a point at `baseX + dir * size`.
const head = (baseX, baseY, dir, size) => {
  for (let i = 0; i <= size; i++) {
    const halfSpan = Math.round((size - i) * 0.85);
    for (let j = -halfSpan; j <= halfSpan; j++) white(baseX + dir * i, baseY + j);
  }
};

const w = 26;
stroke(300, 400, 640, 400, w);
head(640, 400, 1, 86);
stroke(384, 624, 724, 624, w);
head(384, 624, -1, 86);

// --- PNG encoding ---
const raw = Buffer.alloc((S * 4 + 1) * S);
for (let y = 0; y < S; y++) {
  raw[y * (S * 4 + 1)] = 0; // filter: none
  px.copy(raw, y * (S * 4 + 1) + 1, y * S * 4, (y + 1) * S * 4);
}

const chunk = (type, data) => {
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length);
  const body = Buffer.concat([Buffer.from(type, "ascii"), data]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(body) >>> 0);
  return Buffer.concat([len, body, crc]);
};

const ihdr = Buffer.alloc(13);
ihdr.writeUInt32BE(S, 0);
ihdr.writeUInt32BE(S, 4);
ihdr[8] = 8; // bit depth
ihdr[9] = 6; // RGBA
writeFileSync(
  process.argv[2],
  Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk("IHDR", ihdr),
    chunk("IDAT", deflateSync(raw, { level: 9 })),
    chunk("IEND", Buffer.alloc(0)),
  ]),
);
console.log("wrote", process.argv[2]);
