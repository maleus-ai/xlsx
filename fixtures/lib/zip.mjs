// Minimal streaming ZIP writer.
//
// Hand-rolled on purpose: several fixtures are archives that no real library
// would agree to write (three million shared strings, a central directory that
// lies about its sizes). Only `node:zlib` and `node:fs` are used.

import fs from "node:fs";
import zlib from "node:zlib";
import { Readable } from "node:stream";

const LOCAL_SIG = 0x04034b50;
const CENTRAL_SIG = 0x02014b50;
const EOCD_SIG = 0x06054b50;

const METHOD_STORE = 0;
const METHOD_DEFLATE = 8;

// Fixed DOS timestamp: fixtures must be byte-reproducible from one run to the next.
const DOS_TIME = 0; // 00:00:00
const DOS_DATE = ((2026 - 1980) << 9) | (1 << 5) | 1; // 2026-01-01

export class ZipWriter {
  #fd;
  #offset = 0;
  #entries = [];

  constructor(path) {
    this.#fd = fs.openSync(path, "w");
  }

  #write(buffer) {
    fs.writeSync(this.#fd, buffer, 0, buffer.length, this.#offset);
    this.#offset += buffer.length;
  }

  #localHeader(name, method) {
    const nameBuf = Buffer.from(name, "utf8");
    const header = Buffer.alloc(30 + nameBuf.length);
    header.writeUInt32LE(LOCAL_SIG, 0);
    header.writeUInt16LE(20, 4); // version needed
    header.writeUInt16LE(0, 6); // flags
    header.writeUInt16LE(method, 8);
    header.writeUInt16LE(DOS_TIME, 10);
    header.writeUInt16LE(DOS_DATE, 12);
    // crc32 (14), compressed size (18), uncompressed size (22): patched afterwards.
    header.writeUInt16LE(nameBuf.length, 26);
    header.writeUInt16LE(0, 28); // extra length
    nameBuf.copy(header, 30);
    return header;
  }

  #patchLocalHeader(headerOffset, crc, compressedSize, uncompressedSize) {
    const patch = Buffer.alloc(12);
    patch.writeUInt32LE(crc >>> 0, 0);
    patch.writeUInt32LE(compressedSize, 4);
    patch.writeUInt32LE(uncompressedSize, 8);
    fs.writeSync(this.#fd, patch, 0, patch.length, headerOffset + 14);
  }

  /// Add an entry, deflating its content on the fly. `source` is anything
  /// `Readable.from` accepts — a string, a buffer, or a (async) generator.
  async add(name, source, { store = false } = {}) {
    const method = store ? METHOD_STORE : METHOD_DEFLATE;
    const headerOffset = this.#offset;
    this.#write(this.#localHeader(name, method));

    let crc = 0;
    let uncompressedSize = 0;
    let compressedSize = 0;

    const input = Readable.from(source);

    if (store) {
      for await (const chunk of input) {
        const buf = Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk);
        crc = zlib.crc32(buf, crc);
        uncompressedSize += buf.length;
        compressedSize += buf.length;
        this.#write(buf);
      }
    } else {
      const deflate = zlib.createDeflateRaw({ level: 6 });
      input.on("data", (chunk) => {
        const buf = Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk);
        crc = zlib.crc32(buf, crc);
        uncompressedSize += buf.length;
      });
      input.pipe(deflate);
      for await (const chunk of deflate) {
        compressedSize += chunk.length;
        this.#write(chunk);
      }
    }

    this.#patchLocalHeader(headerOffset, crc, compressedSize, uncompressedSize);
    this.#entries.push({
      name,
      method,
      crc,
      compressedSize,
      uncompressedSize,
      headerOffset,
    });
  }

  /// Add an entry whose central directory record deliberately disagrees with
  /// what the entry actually deflates to. `declaredUncompressedSize` is what the
  /// archive claims; the real expansion is whatever `source` produces.
  async addLying(name, source, declaredUncompressedSize) {
    await this.add(name, source);
    const entry = this.#entries[this.#entries.length - 1];
    entry.declaredUncompressedSize = declaredUncompressedSize;
    // The local header lies too, so that a reader trusting either copy is fooled.
    this.#patchLocalHeader(
      entry.headerOffset,
      entry.crc,
      entry.compressedSize,
      declaredUncompressedSize,
    );
  }

  close() {
    const centralOffset = this.#offset;

    for (const entry of this.#entries) {
      const nameBuf = Buffer.from(entry.name, "utf8");
      const record = Buffer.alloc(46 + nameBuf.length);
      record.writeUInt32LE(CENTRAL_SIG, 0);
      record.writeUInt16LE(20, 4); // version made by
      record.writeUInt16LE(20, 6); // version needed
      record.writeUInt16LE(0, 8); // flags
      record.writeUInt16LE(entry.method, 10);
      record.writeUInt16LE(DOS_TIME, 12);
      record.writeUInt16LE(DOS_DATE, 14);
      record.writeUInt32LE(entry.crc >>> 0, 16);
      record.writeUInt32LE(entry.compressedSize, 20);
      record.writeUInt32LE(
        entry.declaredUncompressedSize ?? entry.uncompressedSize,
        24,
      );
      record.writeUInt16LE(nameBuf.length, 28);
      record.writeUInt16LE(0, 30); // extra length
      record.writeUInt16LE(0, 32); // comment length
      record.writeUInt16LE(0, 34); // disk number
      record.writeUInt16LE(0, 36); // internal attributes
      record.writeUInt32LE(0, 38); // external attributes
      record.writeUInt32LE(entry.headerOffset, 42);
      nameBuf.copy(record, 46);
      this.#write(record);
    }

    const eocd = Buffer.alloc(22);
    eocd.writeUInt32LE(EOCD_SIG, 0);
    eocd.writeUInt16LE(0, 4); // disk number
    eocd.writeUInt16LE(0, 6); // central directory disk
    eocd.writeUInt16LE(this.#entries.length, 8);
    eocd.writeUInt16LE(this.#entries.length, 10);
    eocd.writeUInt32LE(this.#offset - centralOffset, 12);
    eocd.writeUInt32LE(centralOffset, 16);
    eocd.writeUInt16LE(0, 20); // comment length
    this.#write(eocd);

    fs.closeSync(this.#fd);
  }
}
