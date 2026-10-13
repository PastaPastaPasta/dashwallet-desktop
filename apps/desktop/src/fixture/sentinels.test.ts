import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { fixtureSentinels, GENERIC, holds, overlap, SHARED } from "../../scripts/fixture-sentinels.mjs";

const app = fileURLToPath(new URL("../..", import.meta.url));

// scripts/check-bundle.mjs fails a production renderer holding any of these (Sol r2 finding 2).
describe("fixture sentinels", () => {
  // SHARED exempts fixture literals the rest of src/ holds too. Reviewed, not computed per build
  // (a fixture copy elsewhere would exempt itself), so it must equal the actual overlap: a new
  // shared literal is added here on purpose, and one no longer shared stops being exempt.
  it("SHARED is exactly the fixture literals the rest of src/ also holds", async () => {
    const { shared } = await overlap(app);
    expect(shared).toEqual([...SHARED].sort());
  });

  // GENERIC exempts fixture literals a dependency could hold; an entry the fixture no longer
  // has is stale.
  it("GENERIC holds only fixture literals", async () => {
    const { fixture } = await overlap(app);
    for (const literal of GENERIC) expect(fixture.has(literal), literal).toBe(true);
  });

  it("include the sample wallet's labels and seed, and no plain word", async () => {
    const sentinels = await fixtureSentinels(app);
    expect(sentinels.size).toBeGreaterThan(50);
    expect(sentinels.get("Coffee refund")).toBe("src/fixture/wallet.ts");
    expect(sentinels.get("#20261006")).toBe("src/fixture/wallet.ts");
    expect(sentinels.has("unlocked")).toBe(false);
    for (const literal of [...SHARED, ...GENERIC]) expect(sentinels.has(literal), literal).toBe(false);
  });

  it("are found in the forms a minifier writes", () => {
    expect(holds('n("Coffee refund")', "Coffee refund")).toBe(true);
    expect(holds("x='it\\'s here'", "it's here")).toBe(true);
    expect(holds('x="say \\"hi\\" now"', 'say "hi" now')).toBe(true);
    expect(holds("seed=20261006,", "#20261006")).toBe(true);
    expect(holds("seed=0x135288E;", "#20261006")).toBe(true);
    expect(holds("h=a20261006b", "#20261006")).toBe(false);
    expect(holds("v=202610061", "#20261006")).toBe(false);
  });
});
