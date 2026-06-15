#!/usr/bin/env node
import { spawnSync } from "node:child_process";
import { mkdtempSync, readdirSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { basename, join } from "node:path";
import { tmpdir } from "node:os";

const [srcDir, outPath, codecArg = "lz4-block", levelArgRaw] = process.argv.slice(2);
if (!srcDir || !outPath) {
  console.error("usage: scripts/build-preview-archive.mjs <raw565-dir> <out> [lz4-block|raw] [level]");
  process.exit(2);
}

const codec = codecArg.toLowerCase();
if (codec !== "lz4-block" && codec !== "raw") {
  console.error("codec must be lz4-block or raw");
  process.exit(2);
}
const levelArg = levelArgRaw ?? "9";
const level = Number.parseInt(levelArg, 10);
if (codec !== "raw" && (!Number.isInteger(level) || level < 1 || level > 12)) {
  console.error(`${codec} level is out of range`);
  process.exit(2);
}

const files = readdirSync(srcDir)
  .filter((name) => name.endsWith(".rgb565"))
  .sort();
if (files.length === 0) {
  console.error(`no .rgb565 files in ${srcDir}`);
  process.exit(1);
}

const tempDir = mkdtempSync(join(tmpdir(), "magik-preview-archive-"));
const entries = [];
const chunks = [];
try {
  for (const name of files) {
    const input = join(srcDir, name);
    const output = join(tempDir, `${name}.lz4`);
    let compressed;
    const rawLen = statSync(input).size;
    if (codec === "raw") {
      compressed = readFileSync(input);
    } else {
      const child = spawnSync("lz4", [`-${level}`, "-q", "-f", input, output], {
        stdio: ["ignore", "pipe", "pipe"],
      });
      if (child.status !== 0) {
        process.stderr.write(child.stderr);
        throw new Error(`${codec} failed for ${input}`);
      }
      compressed = extractLz4BlockPayload(readFileSync(output), rawLen, name);
    }
    const nameBytes = Buffer.from(name, "utf8");
    if (nameBytes.length > 0xffff) {
      throw new Error(`name too long: ${name}`);
    }
    if (rawLen > 0xffffffff || compressed.length > 0xffffffff) {
      throw new Error(`file too large: ${name}`);
    }
    entries.push({ name, nameBytes, rawLen, compressedLen: compressed.length, offset: 0 });
    chunks.push(compressed);
  }

  let indexLen = 8 + 4;
  for (const entry of entries) {
    indexLen += 2 + 4 + 4 + 8 + entry.nameBytes.length;
  }
  let offset = indexLen;
  for (const entry of entries) {
    entry.offset = offset;
    offset += entry.compressedLen;
  }

  const header = Buffer.alloc(indexLen);
  let p = 0;
  header.write(codec === "lz4-block" ? "MMLZ4B1\0" : "MMRAWP1\0", p, "binary");
  p += 8;
  header.writeUInt32LE(entries.length, p);
  p += 4;
  for (const entry of entries) {
    header.writeUInt16LE(entry.nameBytes.length, p);
    p += 2;
    header.writeUInt32LE(entry.rawLen, p);
    p += 4;
    header.writeUInt32LE(entry.compressedLen, p);
    p += 4;
    header.writeBigUInt64LE(BigInt(entry.offset), p);
    p += 8;
    entry.nameBytes.copy(header, p);
    p += entry.nameBytes.length;
  }

  writeFileSync(outPath, Buffer.concat([header, ...chunks]));
  const rawBytes = entries.reduce((sum, entry) => sum + entry.rawLen, 0);
  const compressedBytes = entries.reduce((sum, entry) => sum + entry.compressedLen, 0);
  console.log(
    `preview_archive codec=${codec} entries=${entries.length} raw_bytes=${rawBytes} compressed_payload_bytes=${compressedBytes} archive_bytes=${statSync(outPath).size} level=${level} output=${outPath}`,
  );
} finally {
  rmSync(tempDir, { recursive: true, force: true });
}

function extractLz4BlockPayload(frame, rawLen, name) {
  if (frame.length < 11 || frame.readUInt32LE(0) !== 0x184d2204) {
    throw new Error(`bad lz4 frame for ${name}`);
  }
  let p = 4;
  const flg = frame[p++];
  p++; // BD
  const version = (flg >> 6) & 0x03;
  if (version !== 1) {
    throw new Error(`unsupported lz4 frame version for ${name}`);
  }
  const blockIndependence = (flg & 0x20) !== 0;
  const blockChecksum = (flg & 0x10) !== 0;
  const contentSize = (flg & 0x08) !== 0;
  const contentChecksum = (flg & 0x04) !== 0;
  const dictId = (flg & 0x01) !== 0;
  if (!blockIndependence) {
    throw new Error(`dependent lz4 blocks are not supported for ${name}`);
  }
  if (contentSize) p += 8;
  if (dictId) p += 4;
  p++; // header checksum

  const blocks = [];
  let sawRaw = false;
  while (p + 4 <= frame.length) {
    const blockHeader = frame.readUInt32LE(p);
    p += 4;
    if (blockHeader === 0) break;
    const rawBlock = (blockHeader & 0x80000000) !== 0;
    const blockLen = blockHeader & 0x7fffffff;
    if (p + blockLen > frame.length) {
      throw new Error(`truncated lz4 block for ${name}`);
    }
    blocks.push(frame.subarray(p, p + blockLen));
    sawRaw ||= rawBlock;
    p += blockLen;
    if (blockChecksum) p += 4;
  }
  if (contentChecksum) p += 4;
  if (blocks.length !== 1) {
    throw new Error(`expected one lz4 block for ${name}, got ${blocks.length}`);
  }
  const block = blocks[0];
  if (sawRaw && block.length !== rawLen) {
    throw new Error(`raw lz4 block size mismatch for ${name}`);
  }
  return Buffer.concat([Buffer.from([sawRaw ? 1 : 0]), block]);
}
