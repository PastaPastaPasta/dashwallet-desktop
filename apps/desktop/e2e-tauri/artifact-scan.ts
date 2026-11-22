// The phrase and key check every kept artifact of a testnet run passes before it leaves the box
// (H-04 O4a): the known secrets as plain text (scratch.ts's `leaks`), any run of 12 or more BIP39
// words, and any extended private key (xprv/tprv). PNGs are read through tesseract and matched
// approximately (look-alikes folded, then within a few edits), so a screenshot of a phrase or
// passphrase is caught too; a PNG it cannot read fails the check. OCR is the second line: the first
// is that a flow types a secret only into a masked field (driver.ts fillSecret). Reports a path and
// the kind of hit, never the text. Node built-ins only, so vitest checks it.

import { spawnSync } from "node:child_process";
import { lstatSync, readdirSync, readFileSync } from "node:fs";
import { inflateSync } from "node:zlib";
import { join } from "node:path";
import { BIP39_ENGLISH } from "../src/onboarding/bip39English.ts";
import { leaks } from "./scratch.ts";

const WORDS = new Set(BIP39_ENGLISH);
/** The shortest phrase a wallet takes. */
export const PHRASE_RUN = 12;
const EXTENDED_KEY = /\b[xt]prv[1-9A-HJ-NP-Za-km-z]{100,112}\b/;

/** The longest run of BIP39 words in `text`, case folded, split on white space, quotes, commas and
 * brackets (a JSON word array counts), with other punctuation trimmed. Number tokens (a phrase
 * grid's "1", "12.") neither count nor break a run. */
export function longestPhraseRun(text: string): number {
  let run = 0;
  let best = 0;
  for (const raw of text.toLowerCase().split(/[\s,;"'`[\]{}()]+/)) {
    const t = raw.replace(/^[^a-z]+|[^a-z]+$/g, "");
    if (t && WORDS.has(t)) best = Math.max(best, ++run);
    else if (raw !== "" && !/^[^a-z]*$/.test(raw)) run = 0;
  }
  return best;
}

/** How many distinct words of `phrase` stand in `text` as whole tokens, in any order (a grid read
 * column by column still counts). */
export function phraseWordsIn(text: string, phrase: string): number {
  const tokens = new Set(text.toLowerCase().split(/[^a-z]+/));
  return new Set(phrase.toLowerCase().split(/\s+/).filter((w) => tokens.has(w))).size;
}

/** A screenshot with this many words of one known phrase shows that phrase. */
export const PHRASE_WORDS_SHOWN = 8;

/** What `text` holds that it must not: a phrase-length word run, an extended private key. */
export function suspicious(text: string): string[] {
  const hits: string[] = [];
  if (longestPhraseRun(text) >= PHRASE_RUN) hits.push("bip39-run");
  if (EXTENDED_KEY.test(text)) hits.push("extended-private-key");
  return hits;
}

function files(dir: string): string[] {
  const out: string[] = [];
  const walk = (d: string) => {
    for (const name of readdirSync(d)) {
      const p = join(d, name);
      const st = lstatSync(p, { throwIfNoEntry: false });
      if (st?.isDirectory()) walk(p);
      else if (st?.isFile()) out.push(p);
    }
  };
  walk(dir);
  return out;
}

/** Whether a PNG shows one colour only (so OCR may rightly read nothing). Decodes 8-bit, non-
 * interlaced images; anything else, or a file that does not decode, counts as not one colour. */
export function singleColourPng(png: Buffer): boolean {
  try {
    if (png.toString("latin1", 1, 4) !== "PNG") return false;
    let width = 0;
    let height = 0;
    let channels = 0;
    const idat: Buffer[] = [];
    for (let at = 8; at + 8 <= png.length; ) {
      const len = png.readUInt32BE(at);
      const type = png.toString("latin1", at + 4, at + 8);
      const data = png.subarray(at + 8, at + 8 + len);
      if (type === "IHDR") {
        width = data.readUInt32BE(0);
        height = data.readUInt32BE(4);
        const [depth, colour, , , interlace] = [data[8], data[9], data[10], data[11], data[12]];
        channels = ({ 0: 1, 2: 3, 3: 1, 4: 2, 6: 4 } as Record<number, number>)[colour!] ?? 0;
        if (depth !== 8 || interlace !== 0 || !channels) return false;
      } else if (type === "IDAT") idat.push(data);
      else if (type === "IEND") break;
      at += 12 + len;
    }
    const raw = inflateSync(Buffer.concat(idat));
    const stride = width * channels;
    if (!width || !height || raw.length < height * (stride + 1)) return false;
    let prev = Buffer.alloc(stride);
    let first: Buffer | null = null;
    for (let y = 0; y < height; y++) {
      const filter = raw[y * (stride + 1)];
      const line = Buffer.from(raw.subarray(y * (stride + 1) + 1, (y + 1) * (stride + 1)));
      for (let i = 0; i < stride; i++) {
        const a = i >= channels ? line[i - channels]! : 0;
        const b = prev[i]!;
        const c = i >= channels ? prev[i - channels]! : 0;
        const p = a + b - c;
        const paeth = Math.abs(p - a) <= Math.abs(p - b) && Math.abs(p - a) <= Math.abs(p - c) ? a : Math.abs(p - b) <= Math.abs(p - c) ? b : c;
        const add = [0, a, b, (a + b) >> 1, paeth][filter!];
        if (add === undefined) return false;
        line[i] = (line[i]! + add) & 0xff;
      }
      first ??= line.subarray(0, channels);
      for (let i = 0; i < stride; i += channels) if (!line.subarray(i, i + channels).equals(first)) return false;
      prev = line;
    }
    return true;
  } catch {
    return false;
  }
}

const hasTesseract = () => spawnSync("tesseract", ["--version"], { encoding: "utf8" }).status === 0;

/** How a secret is looked for in OCR text, which breaks lines, splits or joins words and confuses
 * look-alikes: case folded, white space dropped, and each look-alike group as one character:
 * i l j 1 | ! [ ] { } ( ), o 0 q d, s 5 $, z 2, b 8, g 9, - _ ~ and quotes. */
export const folded = (s: string) =>
  s
    .toLowerCase()
    .replace(/\s+/g, "")
    .replace(/[ilj1|!\[\]{}()]/g, "1")
    .replace(/[o0qd]/g, "0")
    .replace(/[s5$]/g, "5")
    .replace(/z/g, "2")
    .replace(/b/g, "8")
    .replace(/g/g, "9")
    .replace(/[-_~]/g, "-")
    .replace(/[`'"‘’“”]/g, "'");

/** A secret this long or longer is also matched approximately in OCR text; a shorter one only
 * exactly (folded), so that short common words do not match. */
export const APPROX_MIN = 8;

/** Whether some window of `text` lies within edit distance max(2, 20% of its length) of `secret`
 * (both folded): Sellers' approximate substring match. */
export function nearMatch(text: string, secret: string): boolean {
  const n = secret.length;
  if (n < APPROX_MIN) return text.includes(secret);
  const k = Math.max(2, Math.floor(n * 0.2));
  // col[i]: the least edits that turn secret[0..i) into a substring of text ending here.
  let col = Array.from({ length: n + 1 }, (_, i) => i);
  for (const c of text) {
    const next = [0];
    for (let i = 1; i <= n; i++) next[i] = Math.min(col[i]! + 1, next[i - 1]! + 1, col[i - 1]! + (secret[i - 1] === c ? 0 : 1));
    if (next[n]! <= k) return true;
    col = next;
  }
  return false;
}

/** Every hit under `dirs`, as `<path>: <kind>`; `ocr` says whether tesseract was there. A PNG that
 * cannot be read (no tesseract, an OCR run that fails, no text read from an image of more than one
 * colour) is a hit: unread is not clean. */
export function scanArtifacts(dirs: string[], secrets: string[]): { hits: string[]; files: number; ocr: boolean } {
  const ocr = hasTesseract();
  const hits: string[] = [];
  const known = [...new Set(secrets.map(folded).filter(Boolean))];
  const phrases = secrets.filter((s) => s.trim().split(/\s+/).length >= PHRASE_RUN);
  let count = 0;
  for (const dir of dirs) {
    hits.push(...leaks(dir, secrets).map((f) => `${f}: known-secret`));
    for (const f of files(dir)) {
      count++;
      if (!f.endsWith(".png")) {
        for (const kind of suspicious(readFileSync(f).toString("latin1"))) hits.push(`${f}: ${kind}`);
        continue;
      }
      if (!ocr) {
        hits.push(`${f}: unread (no tesseract)`);
        continue;
      }
      const r = spawnSync("tesseract", [f, "-", "--psm", "11"], { encoding: "utf8", maxBuffer: 1 << 26 });
      if (r.error || r.status !== 0) {
        hits.push(`${f}: unread (tesseract ${r.error ? r.error.message : `exit ${r.status ?? r.signal}`})`);
        continue;
      }
      const text = r.stdout ?? "";
      if (!text.trim() && !singleColourPng(readFileSync(f))) {
        hits.push(`${f}: unread (tesseract read no text from a screenshot of more than one colour)`);
        continue;
      }
      for (const kind of suspicious(text)) hits.push(`${f}: ${kind}`);
      const flat = folded(text);
      if (known.some((s) => nearMatch(flat, s)) || phrases.some((p) => phraseWordsIn(text, p) >= PHRASE_WORDS_SHOWN)) hits.push(`${f}: known-secret (ocr)`);
    }
  }
  return { hits, files: count, ocr };
}
