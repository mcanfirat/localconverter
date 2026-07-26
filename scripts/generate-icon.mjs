// Generates the LocalConvert source icon: two offset sheets, the back one
// tinted, with a convert arrow knocked out of the front one. Drawn straight
// into an RGBA buffer and zlib-deflated into a PNG, so the icon is
// reproducible from source rather than committed as an opaque binary.
//
//   node scripts/generate-icon.mjs /tmp/icon-source.png
//   cd apps/desktop && pnpm tauri icon /tmp/icon-source.png
//
// `tauri icon` derives every other size, the .icns and the .ico from it.
import { deflateSync, crc32 } from "node:zlib";
import { writeFileSync } from "node:fs";

const S = 1024;
const px = Buffer.alloc(S * S * 4);

// The app's own accent, so the icon and the window agree.
const BG = [22, 21, 19]; // near-black; the sheets need something to sit on
const SHEET = [139, 133, 245]; // --accent, dark-mode value
const BACK_ALPHA = 0.42; // the sheet behind is tinted, not a second hue

const set = (x, y, [r, g, b], a) => {
  if (x < 0 || y < 0 || x >= S || y >= S || a <= 0) return;
  const i = (y * S + x) * 4;
  const sa = a / 255;
  px[i] = Math.round(px[i] * (1 - sa) + r * sa);
  px[i + 1] = Math.round(px[i + 1] * (1 - sa) + g * sa);
  px[i + 2] = Math.round(px[i + 2] * (1 - sa) + b * sa);
  px[i + 3] = Math.min(255, Math.round(px[i + 3] + a * (1 - px[i + 3] / 255)));
};

// Coverage of a rounded rectangle at a pixel centre, in [0,1]. The half-pixel
// band is what keeps the corners from looking like stairs.
const coverage = (x, y, rx, ry, w, h, r) => {
  if (x < rx || y < ry || x > rx + w || y > ry + h) return 0;
  const cx = Math.min(Math.max(x, rx + r), rx + w - r);
  const cy = Math.min(Math.max(y, ry + r), ry + h - r);
  const d = Math.hypot(x - cx, y - cy);
  return Math.max(0, Math.min(1, r - d + 0.5));
};

const fill = (rx, ry, w, h, r, colour, alpha = 1) => {
  for (let y = Math.floor(ry); y <= Math.ceil(ry + h); y++) {
    for (let x = Math.floor(rx); x <= Math.ceil(rx + w); x++) {
      const a = coverage(x, y, rx, ry, w, h, r);
      if (a > 0) set(x, y, colour, Math.round(a * 255 * alpha));
    }
  }
};

// --- background tile -------------------------------------------------------
fill(40, 40, S - 80, S - 80, 200, BG);

// --- the two sheets --------------------------------------------------------
// Geometry mirrors the 64-unit mark drawn in the window header, scaled by 16,
// so the icon and the in-app logo are one drawing at two sizes.
fill(160, 128, 544, 672, 96, SHEET, BACK_ALPHA);
fill(336, 240, 544, 672, 96, SHEET);

// --- arrow, knocked out of the front sheet ---------------------------------
// Painted in the background colour rather than white: the arrow is a hole in
// the sheet, which is what makes the mark read as one object rather than two.
const knock = (x, y, a = 255) => set(Math.round(x), Math.round(y), BG, a);

const bar = (x0, x1, y, w) => {
  for (let x = x0; x <= x1; x++)
    for (let dy = -w; dy <= w; dy++) {
      knock(x, y + dy, Math.abs(dy) > w - 1.5 ? 150 : 255);
    }
};

// Triangular head: a point at `tipX`, widening back towards the bar. `i`
// counts backwards from the tip, so the half-span grows with it — the other
// way round draws a wedge that is widest at the point.
const head = (tipX, y, size) => {
  for (let i = 0; i <= size; i++) {
    const half = Math.round(i * 0.92);
    for (let j = -half; j <= half; j++) knock(tipX - i, y + j);
  }
};

// The front sheet spans x 336..880, y 240..912, so its centre is (608, 576).
// The arrow is laid out about that centre, and the bar runs a little past the
// head's base so the two read as one shape rather than two touching ones.
const MID = 576;
bar(500, 652, MID, 21);
head(716, MID, 68);

// --- PNG encoding ----------------------------------------------------------
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
