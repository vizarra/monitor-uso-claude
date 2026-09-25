// Genera `src-tauri/app-icon.png` (1024x1024): el icono propio de la app, un
// anillo de progreso al 70 %. Sin dependencias: escribe el PNG a mano.
// Después, `npx tauri icon src-tauri/app-icon.png` crea todos los tamaños.
//
// Uso: node scripts/gen-app-icon.mjs

import { writeFileSync } from "node:fs";
import { deflateSync } from "node:zlib";

const SIZE = 1024;
const SS = 4; // submuestreo por eje para suavizar los bordes
const CENTER = SIZE / 2;
const OUTER = 470;
const INNER = 330;
const PROGRESS = 0.7;
const TRACK = [203, 213, 225];
const FILL = [37, 99, 235];

function sample(x, y) {
  const dx = x - CENTER;
  const dy = y - CENTER;
  const r = Math.hypot(dx, dy);
  if (r > OUTER || r < INNER) return null;
  // Ángulo desde las 12 en sentido horario, en fracción de vuelta.
  const turn = (Math.atan2(dx, -dy) / (2 * Math.PI) + 1) % 1;
  return turn <= PROGRESS ? FILL : TRACK;
}

const raw = Buffer.alloc(SIZE * (SIZE * 4 + 1));
for (let y = 0; y < SIZE; y++) {
  const row = y * (SIZE * 4 + 1);
  raw[row] = 0; // filtro "None"
  for (let x = 0; x < SIZE; x++) {
    let r = 0, g = 0, b = 0, a = 0;
    for (let sy = 0; sy < SS; sy++) {
      for (let sx = 0; sx < SS; sx++) {
        const c = sample(x + (sx + 0.5) / SS, y + (sy + 0.5) / SS);
        if (c) { r += c[0]; g += c[1]; b += c[2]; a++; }
      }
    }
    const i = row + 1 + x * 4;
    if (a > 0) {
      raw[i] = Math.round(r / a);
      raw[i + 1] = Math.round(g / a);
      raw[i + 2] = Math.round(b / a);
    }
    raw[i + 3] = Math.round((a / (SS * SS)) * 255);
  }
}

const crcTable = Array.from({ length: 256 }, (_, n) => {
  let c = n;
  for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
  return c >>> 0;
});
function crc32(buf) {
  let c = 0xffffffff;
  for (const byte of buf) c = crcTable[(c ^ byte) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}
function chunk(type, data) {
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length);
  const body = Buffer.concat([Buffer.from(type, "ascii"), data]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(body));
  return Buffer.concat([len, body, crc]);
}

const ihdr = Buffer.alloc(13);
ihdr.writeUInt32BE(SIZE, 0);
ihdr.writeUInt32BE(SIZE, 4);
ihdr[8] = 8; // bits por canal
ihdr[9] = 6; // RGBA
const png = Buffer.concat([
  Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
  chunk("IHDR", ihdr),
  chunk("IDAT", deflateSync(raw)),
  chunk("IEND", Buffer.alloc(0)),
]);
writeFileSync(new URL("../src-tauri/app-icon.png", import.meta.url), png);
console.log("src-tauri/app-icon.png generado");
