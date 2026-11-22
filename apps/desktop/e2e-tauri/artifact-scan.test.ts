import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { longestPhraseRun, scanArtifacts, suspicious } from "./artifact-scan.ts";

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
