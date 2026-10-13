import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { distinctive, fixtureSentinels, generatedLiterals, GENERIC, holds, literalsOf, NO_SENTINEL } from "../../scripts/fixture-sentinels.mjs";

const app = fileURLToPath(new URL("../..", import.meta.url));

// Constants published with a PRNG, which a dependency may use as well.
const PRNG_CONSTANTS = new Set([0x6d2b79f5 /* mulberry32 */]);

/** Whether a GENERIC literal has a shape no fixture data has: a power of two, a repdigit, a run of
 * consecutive digits, a published PRNG constant, or an alphabet (16 or more characters in strictly
 * ascending order). */
function genericShape(literal: string): boolean {
  const number = /^#(\d+)$/.exec(literal);
  if (number) {
    const n = BigInt(number[1]);
    return (n & (n - 1n)) === 0n || /^(\d)\1+$/.test(number[1]) || "0123456789".includes(number[1]) || PRNG_CONSTANTS.has(Number(n));
  }
  return literal.length >= 16 && [...literal].every((c, i) => i === 0 || c > literal[i - 1]);
}

// scripts/check-bundle.mjs fails a production renderer holding any of these (Sol r2 finding 2,
// r3 findings 1 and 2).
describe("fixture sentinels", () => {
  // The only shared literals are the generated bindings': an error code the fixture returns is
  // not one, a fixture message is, wherever else a copy of it is (Sol r3: an `errors.ts` copy
  // outside src/fixture/ exempts nothing).
  it("leave out exactly the literals the generated bindings hold", async () => {
    const generated = await generatedLiterals(app);
    const { sentinels } = await fixtureSentinels(app);
    expect(generated.has("vault.wrong_passphrase")).toBe(true);
    expect(sentinels.has("vault.wrong_passphrase")).toBe(false);
    expect(generated.has("Block height out of range")).toBe(false);
    expect(sentinels.get("Block height out of range")).toBe("src/fixture/errors.ts");
    for (const literal of sentinels.keys()) expect(generated.has(literal), literal).toBe(false);
  });

  // GENERIC exempts by hand, so each entry says why and has a shape the fixture's own data does
  // not: the sample wallet's seed or labels cannot be listed.
  it("GENERIC lists only generic shapes, each with its reason, all fixture literals", async () => {
    const { byModule } = await fixtureSentinels(app);
    const fixture = new Set<string>();
    for (const module of byModule.keys()) for (const literal of await literalsOf(`${app}/${module}`)) fixture.add(literal);
    expect(GENERIC.length).toBeLessThanOrEqual(6);
    for (const { literal, reason } of GENERIC) {
      expect(reason.length, literal).toBeGreaterThan(8);
      expect(genericShape(literal), literal).toBe(true);
      expect(distinctive(literal) && fixture.has(literal), literal).toBe(true);
    }
    for (const literal of ["#20261006", "Coffee refund", "#1831565812", "ACEGIKMOQSUWY"]) expect(genericShape(literal), literal).toBe(false);
  });

  it("NO_SENTINEL lists only fixture modules without a sentinel, each with its reason", async () => {
    const { byModule } = await fixtureSentinels(app);
    for (const [module, reason] of Object.entries(NO_SENTINEL)) {
      expect(byModule.get(`src/fixture/${module}`), module).toEqual([]);
      expect(reason.length, module).toBeGreaterThan(8);
    }
  });

  it("include the sample wallet's labels and seed, and no plain word", async () => {
    const { sentinels } = await fixtureSentinels(app);
    expect(sentinels.size).toBeGreaterThan(50);
    expect(sentinels.get("Coffee refund")).toBe("src/fixture/wallet.ts");
    expect(sentinels.get("#20261006")).toBe("src/fixture/wallet.ts");
    for (const word of ["unlocked", " failed)", " characters", "fixture: "]) expect(sentinels.has(word), word).toBe(false);
    for (const { literal } of GENERIC) expect(sentinels.has(literal), literal).toBe(false);
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
