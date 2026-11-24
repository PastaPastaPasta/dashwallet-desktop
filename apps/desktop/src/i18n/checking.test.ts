import { describe, expect, it } from "vitest";
import { shieldResultText } from "../coinjoin/move";
import { FUND_COPY, fundRowView } from "../platform/fund";
import { RECOVERY_COPY } from "../recovery/model";
import { SHIELDED_COPY } from "../shielded/flow";
import { flatten } from "./core";
import { en } from "./en";

describe("the Checking lines", () => {
  it("never tell the user to wait: no catalog string says we can't confirm it yet", () => {
    const texts = [...flatten(en).values()].flatMap((m) => (typeof m === "string" ? [m] : Object.values(m)));
    expect(texts.filter((m) => /can't confirm it yet/i.test(m))).toEqual([]);
  });

  it("never promise that Check again adds the amount: it tries to", () => {
    const texts = [...flatten(en).values()].flatMap((m) => (typeof m === "string" ? [m] : Object.values(m)));
    expect(texts.filter((m) => m.includes("Check again adds it"))).toEqual([]);
  });

  it("name the Home row's own Check again button", () => {
    for (const copy of [FUND_COPY, SHIELDED_COPY]) {
      const label = copy.card.checkAgain;
      expect(copy.done.checking.row).toContain(`Choose ${label} to confirm it.`);
      expect(copy.done.checking.row).toContain(`, ${label} tries to add it.`);
    }
  });

  it("on a result with no button, name Home's Unfinished transfers card and its Check again button", () => {
    for (const copy of [FUND_COPY, SHIELDED_COPY]) {
      const label = copy.card.checkAgain;
      expect(copy.done.checking.text).toContain(`choose ${label} on the ${RECOVERY_COPY.title} card on Home.`);
      expect(copy.done.checking.text).toContain(`, ${label} tries to add it.`);
    }
  });

  it("go to the right screens: a move's result names Home, Home's row names its button", () => {
    const result = shieldResultText({ txid: "aa".repeat(32), state: { kind: "consumptionUnknown" } }, (c) => c);
    expect([result.title, result.text]).toEqual([SHIELDED_COPY.done.checking.title, SHIELDED_COPY.done.checking.text]);
    const row = fundRowView({ duffs: 50_000_000, state: "checking", consumptionUnknown: true }, String);
    expect(row).toEqual({ kind: "checking", text: FUND_COPY.done.checking.row, action: "checkAgain" });
  });
});

describe("the close-this lines", () => {
  it("say one thing in one way: every running step that lets the user close it reads the same sentence", () => {
    const all = flatten(en);
    const keys = ["transfer.move.running.text", "transfer.fund.running.text", "transfer.shielded.running.prove", "transfer.shielded.running.lock", "pay.platform.running.text"];
    expect(keys.filter((k) => !String(all.get(k)).endsWith("You can close this: Activity shows it."))).toEqual([]);
    const texts = [...all.values()].flatMap((m) => (typeof m === "string" ? [m] : Object.values(m)));
    expect(texts.filter((m) => /close this\W+Activity/i.test(m) && !m.endsWith("You can close this: Activity shows it."))).toEqual([]);
  });
});
