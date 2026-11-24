import { describe, expect, it } from "vitest";
import { fromErrorText, fromRows, liveSources, outcomeText, payingSource, type FromInputs } from "./fromBalance";

const amount = (d: number) => `${d / 1e8} tDASH`;
const credits = (c: string) => amount(Number(BigInt(c) / 1000n));
const base: FromInputs = { coreDuffs: 5e8, shielded: 2e8, platform: 3e8, amount, discreet: false, acceptsToo: false };

describe("the From step (C-funding P16)", () => {
  it("lists Dash wallet, Shielded and Platform, each with its balance", () => {
    expect(fromRows(base).map((r) => [r.source, r.label, r.balance])).toEqual([
      ["core", "Dash wallet", "5 tDASH"],
      ["shielded", "Shielded balance", "2 tDASH"],
      ["platform", "Platform balance", "3 tDASH"],
    ]);
  });

  it("lists a source that cannot pay with its reason, never hides it", () => {
    const rows = fromRows({ ...base, platform: "absent" });
    expect(rows.find((r) => r.source === "shielded")?.reason).toBe("Not available on this network yet.");
    expect(rows.find((r) => r.source === "platform")?.reason).toBe("Nothing to pay with here.");
    expect(rows.find((r) => r.source === "core")?.reason).toBeNull();
    expect(rows).toHaveLength(3);
  });

  it("keeps the balances off on mainnet", () => {
    const rows = fromRows({ ...base, live: liveSources("mainnet") });
    expect(rows.map((r) => r.reason)).toEqual([null, "Not available on this network yet.", "Not available on this network yet."]);
    expect(payingSource(rows, "platform")).toBe("core");
    expect(fromRows({ ...base, live: liveSources("testnet") }).find((r) => r.source === "platform")?.reason).toBeNull();
  });

  it("leaves Accept and pay to the Dash wallet", () => {
    expect(fromRows({ ...base, acceptsToo: true }).find((r) => r.source === "platform")?.reason).toBe("Accept and pay uses your Dash wallet.");
  });

  it("shows no balance in discreet mode or while unread", () => {
    expect(fromRows({ ...base, discreet: true }).map((r) => r.balance)).toEqual([null, null, null]);
    expect(fromRows({ ...base, platform: "loading" }).find((r) => r.source === "platform")).toMatchObject({ balance: null, reason: null });
  });

  it("pays from the Dash wallet when the chosen source cannot pay", () => {
    expect(payingSource(fromRows(base), "platform")).toBe("platform");
    expect(payingSource(fromRows({ ...base, platform: 0 }), "platform")).toBe("core");
    expect(payingSource(fromRows(base), "shielded")).toBe("core");
  });
});

describe("a payment from a balance's copy (D-C5 Copy)", () => {
  it("says the outcome", () => {
    expect(outcomeText("sent", "@bob").text).toBe("Payment sent. It arrives in @bob’s wallet once the Dash network processes it. This can take a while.");
    expect(outcomeText("pending", "@bob")).toEqual({ title: "Payment status unknown", text: "We could not confirm this payment. It may still go through. Do not pay again until this is settled." });
    expect(outcomeText("notSent", "@bob").title).toBe("Payment not sent");
  });

  it("names the minimum below it, and a pending Platform transfer", () => {
    const range = { code: "credits.withdrawal_out_of_range", message: "", data: { kind: "creditRange", min: "1190000", max: "50000000000000" } };
    expect(fromErrorText(range, "1000", credits)).toBe("The smallest payment from here is 0.0000119 tDASH. Pay a smaller amount from your Dash wallet.");
    expect(fromErrorText({ code: "credits.move_pending", message: "", data: null }, "1", credits)).toBe("Another Platform transfer is still being confirmed. Try again when it is done.");
    expect(fromErrorText({ code: "platform.feature_off", message: "", data: null }, "1", credits)).toBe("Not available on this network yet.");
  });
});
