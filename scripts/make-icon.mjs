#!/usr/bin/env node
import { writeFileSync } from "node:fs";
import { deflateSync } from "node:zlib";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const BG = [0x0b, 0x0f, 0x14];
const FG = [0x34, 0xd3, 0x99];
const SIZES = [256, 128, 64, 48, 32, 16];

const S = 0.94;
const t = (v) => (v - 256) * S + 256;
const roundedRect = (x, y, w, h, r) => (px, py) => {
  if (px < x || py < y || px > x + w || py > y + h) return false;
  if (r === 0) return true;
  const dx = Math.max(x + r - px, 0, px - (x + w - r));
  const dy = Math.max(y + r - py, 0, py - (y + h - r));
  return dx * dx + dy * dy <= r * r;
};
const scaledRect = (x, y, w, h) => roundedRect(t(x), t(y), w * S, h * S, 0);
const circle = (cx, cy, r) => {
  const px0 = t(cx);
  const py0 = t(cy);
  const pr = r * S;
  return (px, py) => (px - px0) ** 2 + (py - py0) ** 2 <= pr * pr;
};

const tile = roundedRect(16, 16, 480, 480, 112);

function markShapes(size) {
  const dot = (r) => ({ test: circle(256, 256, r), color: FG });
  if (size < 48) return [dot(112)];
  return [
    { test: scaledRect(140, 140, 84, 40), color: FG },
    { test: scaledRect(140, 140, 40, 84), color: FG },
    { test: scaledRect(288, 140, 84, 40), color: FG },
    { test: scaledRect(332, 140, 40, 84), color: FG },
    { test: scaledRect(140, 332, 84, 40), color: FG },
    { test: scaledRect(140, 288, 40, 84), color: FG },
    { test: scaledRect(288, 332, 84, 40), color: FG },
    { test: scaledRect(332, 288, 40, 84), color: FG },
    dot(40),
  ];
}

function render(size, supersample = 4) {
  const hi = size * supersample;
  const shapes = [{ test: tile, color: BG }, ...markShapes(size)];
  const pixels = new Uint8ClampedArray(size * size * 4);
  for (let y = 0; y < size; y += 1) {
    for (let x = 0; x < size; x += 1) {
      let r = 0, g = 0, b = 0, a = 0;
      for (let sy = 0; sy < supersample; sy += 1) {
        for (let sx = 0; sx < supersample; sx += 1) {
          const px = ((x * supersample + sx + 0.5) / hi) * 512;
          const py = ((y * supersample + sy + 0.5) / hi) * 512;
          let hit = null;
          for (const shape of shapes) if (shape.test(px, py)) hit = shape.color;
          if (hit) {
            r += hit[0];
            g += hit[1];
            b += hit[2];
            a += 255;
          }
        }
      }
      const n = supersample * supersample;
      const i = (y * size + x) * 4;
      const cover = a / n;
      pixels[i] = cover ? Math.round(r / (a / 255)) : 0;
      pixels[i + 1] = cover ? Math.round(g / (a / 255)) : 0;
      pixels[i + 2] = cover ? Math.round(b / (a / 255)) : 0;
      pixels[i + 3] = Math.round(cover);
    }
  }
  return pixels;
}

const crcTable = Array.from({ length: 256 }, (_, n) => {
  let c = n;
  for (let k = 0; k < 8; k += 1) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
  return c >>> 0;
});
const crc32 = (buf) => {
  let c = 0xffffffff;
  for (const byte of buf) c = crcTable[(c ^ byte) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
};

function chunk(type, data) {
  const length = Buffer.alloc(4);
  length.writeUInt32BE(data.length);
  const body = Buffer.concat([Buffer.from(type, "ascii"), data]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(body));
  return Buffer.concat([length, body, crc]);
}

function png(size) {
  const pixels = render(size);
  const raw = Buffer.alloc((size * 4 + 1) * size);
  for (let y = 0; y < size; y += 1) {
    raw[y * (size * 4 + 1)] = 0;
    Buffer.from(pixels.buffer, y * size * 4, size * 4).copy(raw, y * (size * 4 + 1) + 1);
  }
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(size, 0);
  ihdr.writeUInt32BE(size, 4);
  ihdr[8] = 8;
  ihdr[9] = 6;
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk("IHDR", ihdr),
    chunk("IDAT", deflateSync(raw, { level: 9 })),
    chunk("IEND", Buffer.alloc(0)),
  ]);
}

function ico(images) {
  const header = Buffer.alloc(6);
  header.writeUInt16LE(0, 0);
  header.writeUInt16LE(1, 2);
  header.writeUInt16LE(images.length, 4);
  let offset = 6 + images.length * 16;
  const entries = images.map(({ size, data }) => {
    const entry = Buffer.alloc(16);
    entry[0] = size >= 256 ? 0 : size;
    entry[1] = size >= 256 ? 0 : size;
    entry.writeUInt16LE(1, 4);
    entry.writeUInt16LE(32, 6);
    entry.writeUInt32LE(data.length, 8);
    entry.writeUInt32LE(offset, 12);
    offset += data.length;
    return entry;
  });
  return Buffer.concat([header, ...entries, ...images.map((i) => i.data)]);
}

const master = png(512);
writeFileSync(join(root, "assets", "icon.png"), master);
const images = SIZES.map((size) => ({ size, data: png(size) }));
writeFileSync(join(root, "assets", "hand.ico"), ico(images));
console.log(`icon.png ${master.length} bytes; hand.ico ${SIZES.join("/")}`);
