import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { canonical, inside, leaks, newDirIn, removeOwned, scratchRoot } from "./scratch.ts";

const SECRET = "not a real phrase but long enough";

describe("the probe's scratch boundary (BL-162)", () => {
  let base: string;
  beforeEach(() => {
    base = canonical(mkdtempSync(join(tmpdir(), "dw-scratch-test-")));
  });
  afterEach(() => rmSync(base, { recursive: true, force: true }));

  it("knows strictly below from beside, above and lookalike siblings", () => {
    expect(inside("/a/b", "/a/b/c")).toBe(true);
    expect(inside("/a/b", "/a/b")).toBe(false);
    expect(inside("/a/b", "/a")).toBe(false);
    expect(inside("/a/b", "/a/bc")).toBe(false);
    expect(inside("/a/b", "/a/b/../c")).toBe(false);
  });

  it("refuses a root overlapping the checkout, and a path through a symlink out of the root", () => {
    const checkout = join(base, "repo");
    mkdirSync(checkout);
    expect(() => scratchRoot(join(checkout, "scratch"), checkout)).toThrow(/overlap the checkout/);
    expect(existsSync(join(checkout, "scratch")), "refused before it is created").toBe(false);
    expect(() => scratchRoot(checkout, checkout)).toThrow(/overlap the checkout/);
    expect(() => scratchRoot(base, checkout), "a root holding the checkout").toThrow(/overlap the checkout/);
    const root = scratchRoot(join(base, "root"), checkout);
    const outside = join(base, "elsewhere");
    mkdirSync(outside);
    symlinkSync(outside, join(root, "link"));
    expect(() => newDirIn(root, join(root, "link", "out"))).toThrow(/outside the scratch root/);
    expect(() => newDirIn(root, join(root, "..", "elsewhere", "out"))).toThrow(/outside the scratch root/);
    expect(existsSync(join(outside, "out"))).toBe(false);
  });

  it("creates only a new directory below the root", () => {
    const root = scratchRoot(join(base, "root"), join(base, "repo"));
    const out = newDirIn(root, join(root, "w5", "out"));
    expect(existsSync(out)).toBe(true);
    expect(() => newDirIn(root, out)).toThrow(/exists/);
  });

  it("does not follow a directory symlink when it scans, and deletes only what the run owns", () => {
    const owned = join(base, "out");
    const extra = join(base, "fixture");
    mkdirSync(owned);
    mkdirSync(extra);
    writeFileSync(join(owned, "log.txt"), `before ${SECRET} after`);
    writeFileSync(join(extra, "A.mnemonic"), SECRET);
    symlinkSync(extra, join(owned, "fixture-link"));
    symlinkSync(join(extra, "A.mnemonic"), join(owned, "file-link"));

    const inOwned = leaks(owned, [SECRET]);
    expect(inOwned).toEqual([`${join(owned, "log.txt")} (0)`]);
    const inExtra = leaks(extra, [SECRET]);
    expect(inExtra).toEqual([`${join(extra, "A.mnemonic")} (0)`]);

    const kept = removeOwned([...inOwned, ...inExtra], [owned]);
    expect(kept).toEqual(inExtra);
    expect(existsSync(join(owned, "log.txt"))).toBe(false);
    expect(readFileSync(join(extra, "A.mnemonic"), "utf8")).toBe(SECRET);
  });
});
