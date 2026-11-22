// The phrase and key check every kept artifact of a testnet run passes before it leaves the box
// (H-04 O4a): the known secrets as plain text (scratch.ts's `leaks`), any run of 12 or more BIP39
// words, and any extended private key (xprv/tprv). PNGs are read through tesseract when it is
// installed, so a screenshot of a phrase is caught too. Reports a path and the kind of hit, never
// the text. Node built-ins only, so vitest checks it.

import { spawnSync } from "node:child_process";
import { lstatSync, readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { BIP39_ENGLISH } from "../src/onboarding/bip39English.ts";
import { leaks } from "./scratch.ts";

const WORDS = new Set(BIP39_ENGLISH);
/** The shortest phrase a wallet takes. */
export const PHRASE_RUN = 12;
const EXTENDED_KEY = /\b[xt]prv[1-9A-HJ-NP-Za-km-z]{100,112}\b/;

/** The longest run of consecutive BIP39 words in `text`, tokens split on white space with
 * punctuation trimmed from each end (`"abandon,` counts). */
export function longestPhraseRun(text: string): number {
  let run = 0;
  let best = 0;
  for (const raw of text.split(/\s+/)) {
    const t = raw.replace(/^[^a-z]+|[^a-z]+$/g, "");
    if (t && WORDS.has(t)) best = Math.max(best, ++run);
    else if (raw !== "") run = 0;
  }
  return best;
}

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

const hasTesseract = () => spawnSync("tesseract", ["--version"], { encoding: "utf8" }).status === 0;

/** Every hit under `dirs`, as `<path>: <kind>`; `ocr` says whether PNGs were read. */
export function scanArtifacts(dirs: string[], secrets: string[]): { hits: string[]; files: number; ocr: boolean } {
  const ocr = hasTesseract();
  const hits: string[] = [];
  let count = 0;
  for (const dir of dirs) {
    hits.push(...leaks(dir, secrets).map((f) => `${f}: known-secret`));
    for (const f of files(dir)) {
      count++;
      const text = f.endsWith(".png")
        ? ocr
          ? (spawnSync("tesseract", [f, "-", "--psm", "11"], { encoding: "utf8", maxBuffer: 1 << 26 }).stdout ?? "").toLowerCase()
          : ""
        : readFileSync(f).toString("latin1");
      for (const kind of suspicious(text)) hits.push(`${f}: ${kind}`);
      if (f.endsWith(".png") && ocr && secrets.some((s) => s.includes(" ") && text.replace(/\s+/g, " ").includes(s.split(/\s+/).slice(0, 4).join(" "))))
        hits.push(`${f}: known-secret (ocr)`);
    }
  }
  return { hits, files: count, ocr };
}
