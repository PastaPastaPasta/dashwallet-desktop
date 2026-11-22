import { chmodSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { longestPhraseRun, phraseWordsIn, scanArtifacts, suspicious } from "./artifact-scan.ts";

// The BIP39 test vector phrase, public.
const VECTOR = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

describe("the testnet artifact check (H-04 O4a)", () => {
  let dir: string;
  beforeEach(() => {
    dir = mkdtempSync(join(tmpdir(), "dw-artifact-scan-"));
  });
  afterEach(() => rmSync(dir, { recursive: true, force: true }));

  it("counts BIP39 word runs across punctuation, not through other words", () => {
    expect(longestPhraseRun(`"${VECTOR}",`)).toBe(12);
    expect(longestPhraseRun("abandon ability able zzz about above")).toBe(3);
    expect(longestPhraseRun("contacts: the bell lists one request")).toBeLessThan(12);
  });

  it("reads a numbered phrase grid, capitalised words and a JSON word array as one run", () => {
    const words = VECTOR.split(" ");
    expect(longestPhraseRun(words.map((w, i) => `${i + 1} ${w}`).join("\n"))).toBe(12);
    expect(longestPhraseRun(words.map((w, i) => `${i + 1}. ${w}`).join(" "))).toBe(12);
    expect(longestPhraseRun(`Abandon ${words.slice(1).join(" ")}`)).toBe(12);
    expect(longestPhraseRun(JSON.stringify(words))).toBe(12);
  });

  it("counts a known phrase's words in any order", () => {
    const phrase = "legal winner thank year wave sausage worth useful legal winner thank yellow";
    const columns = "1 legal 4 year 7 worth 10 winner\n2 winner 5 wave 8 useful 11 thank\n3 thank 6 sausage 9 legal 12 yellow";
    expect(phraseWordsIn(columns, phrase)).toBe(9);
    expect(phraseWordsIn("Welcome to DashPay, @dwo4aa2257", phrase)).toBe(0);
  });

  it("flags a phrase-length run and an extended private key, and nothing in ordinary logs", () => {
    expect(suspicious(VECTOR)).toEqual(["bip39-run"]);
    expect(suspicious(`key tprv${"8".repeat(107)}`)).toEqual(["extended-private-key"]);
    expect(suspicious("registration: SPV at 1596581; paying 10000000 duffs from A to yXyz")).toEqual([]);
  });

  it("reports the path and the kind, never the text", () => {
    mkdirSync(join(dir, "logs"));
    writeFileSync(join(dir, "logs", "run.log"), `step\n${VECTOR}\n`);
    writeFileSync(join(dir, "clean.json"), '{"ok":true}');
    const r = scanArtifacts([dir], ["a known secret value"]);
    expect(r.hits).toEqual([`${join(dir, "logs", "run.log")}: bip39-run`]);
    expect(r.hits.join("")).not.toContain("abandon");
    writeFileSync(join(dir, "clean.json"), "a known secret value");
    expect(scanArtifacts([dir], ["a known secret value"]).hits).toContain(`${join(dir, "clean.json")} (0): known-secret`);
  });

  describe("screenshots", () => {
    const path = process.env.PATH;
    // A stand-in tesseract: --version answers, a read prints $FAKE_OCR_TEXT and exits $FAKE_OCR_EXIT.
    const fakeTesseract = () => {
      const bin = join(dir, "bin");
      mkdirSync(bin);
      writeFileSync(join(bin, "tesseract"), '#!/bin/sh\n[ "$1" = --version ] && exit 0\nprintf "%s\\n" "$FAKE_OCR_TEXT"\nexit "${FAKE_OCR_EXIT:-0}"\n');
      chmodSync(join(bin, "tesseract"), 0o755);
      process.env.PATH = `${bin}:${path}`;
    };
    const shot = () => {
      mkdirSync(join(dir, "shots"));
      writeFileSync(join(dir, "shots", "1-form.png"), "not really a png");
      return join(dir, "shots");
    };
    afterEach(() => {
      process.env.PATH = path;
      delete process.env.FAKE_OCR_TEXT;
      delete process.env.FAKE_OCR_EXIT;
    });

    it.skipIf(process.platform === "win32")("counts a screenshot it could not read as a hit: no tesseract, or a failed read", () => {
      const shots = shot();
      process.env.PATH = join(dir, "empty");
      expect(scanArtifacts([shots], ["x"]).hits).toEqual([`${join(shots, "1-form.png")}: unread (no tesseract)`]);
      fakeTesseract();
      process.env.FAKE_OCR_EXIT = "1";
      expect(scanArtifacts([shots], ["x"]).hits).toEqual([`${join(shots, "1-form.png")}: unread (tesseract exit 1)`]);
      process.env.FAKE_OCR_EXIT = "0";
      process.env.FAKE_OCR_TEXT = "Pay QA B";
      expect(scanArtifacts([shots], ["x"]).hits).toEqual([]);
    });

    it.skipIf(process.platform === "win32")("finds a known passphrase or phrase in a screenshot whatever its case and line breaks", () => {
      const shots = shot();
      fakeTesseract();
      const pass = "Zq7xK2mPw9";
      const hit = [`${join(shots, "1-form.png")}: known-secret (ocr)`];
      for (const text of [`Passphrase ${pass}`, `passphrase\nzq7xk2\nmpw9`, "ZQ7XK2MPW9"]) {
        process.env.FAKE_OCR_TEXT = text;
        expect(scanArtifacts([shots], [pass]).hits, text).toEqual(hit);
      }
      // A base64url passphrase read with look-alikes swapped.
      for (const text of ["k1O-x0Il", "Kl0_XOI1"]) {
        process.env.FAKE_OCR_TEXT = text;
        expect(scanArtifacts([shots], ["kIo_xOl1"]).hits, text).toEqual(hit);
      }
      process.env.FAKE_OCR_TEXT = "legal winner thank\nyear wave sausage";
      expect(scanArtifacts([shots], ["legal winner thank year wave sausage"]).hits).toEqual(hit);
    });
  });
});
