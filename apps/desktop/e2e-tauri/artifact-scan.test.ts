import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
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
});
