import { describe, expect, it } from "vitest";
import { outpointProblem } from "./outpoint.ts";

const coin = { outpoint: { txid: "ab".repeat(32), vout: 1 }, address: "yNwHG9D2rnrJRQ8j7TG6xnWfbr8D634HeM", amount: 874_999_774 };
const out = { value: 8.74999774, scriptPubKey: { address: coin.address } };

describe("a recorded coin checked against dashd's unspent output (#101)", () => {
  it("accepts the same amount and address, in either scriptPubKey shape", () => {
    expect(outpointProblem(coin, out)).toBeUndefined();
    expect(outpointProblem(coin, { value: out.value, scriptPubKey: { addresses: [coin.address] } })).toBeUndefined();
  });

  it("refuses a spent or unknown output, another amount, or another address", () => {
    expect(outpointProblem(coin, null)).toMatch(/no unspent output/);
    expect(outpointProblem(coin, { ...out, value: 8.74999775 })).toMatch(/874999775 duffs/);
    expect(outpointProblem(coin, { ...out, scriptPubKey: { address: "yLv1zMsGdUAp1Y3m6YqMWvDJUPKn5jY3xW" } })).toMatch(/pays yLv1/);
    expect(outpointProblem(coin, { ...out, scriptPubKey: {} })).toMatch(/no single address/);
    expect(outpointProblem(coin, { ...out, scriptPubKey: { addresses: [coin.address, coin.address] } })).toMatch(/no single address/);
  });
});
