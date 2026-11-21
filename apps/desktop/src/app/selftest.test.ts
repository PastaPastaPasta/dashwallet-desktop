import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { selftestBlocked } from "./selftest";
import { SELFTEST_LOCKED } from "./selftestText";

describe("the self-test of a locked vault", () => {
  it("ends at once when the vault is locked and it was given no way to unlock it", () => {
    expect(selftestBlocked("locked", false)).toBe(SELFTEST_LOCKED);
    expect(selftestBlocked("locked", true)).toBeNull();
    expect(selftestBlocked("unlocked", false)).toBeNull();
    expect(selftestBlocked("noVault", false)).toBeNull();
  });

  it("is the error the CI judge's own test fails a run on", () => {
    const judgeTest = readFileSync(new URL("../../../../ci/github/selftest-judge-test.sh", import.meta.url), "utf8");
    expect(judgeTest).toContain(SELFTEST_LOCKED);
  });
});
