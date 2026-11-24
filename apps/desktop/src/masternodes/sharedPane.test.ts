// A confirmed shared masternode's pane lines (shared.ts, MG-10 §2.4, §2.8 Shown and Validity).

import { describe, expect, it } from "vitest";
import type { ShareFact } from "../api/generated/ShareFact";
import { confirmedCoordinator, sharedShares, SH_MN, SH_REGISTERED, standbyPair } from "../fixture/shared";
import {
  attemptLine,
  earlyEnd,
  eligibilityLine,
  heldShares,
  isSharedRow,
  payoutAddress,
  percentText,
  registrationSession,
  shareLegend,
  standbyLines,
  standbySource,
} from "./shared";

const COIN = 100_000_000;
const dash = (d: number) => `${d / COIN} Dash`;
const short = (a: string) => `${a.slice(0, 4)}…`;
const date = (secs: number) => `t${secs}`;
const alice = (reward: Partial<ShareFact["reward"]> = {}) => sharedShares({}, reward).shares[0];

describe("shared masternode pane", () => {
  it("reads a row as shared when it holds shares, or the engine marks it and it is the wallet's", () => {
    expect(isSharedRow({ shares: { held: 1, of: 3 }, sharedUnsupported: false, mine: false })).toBe(true);
    expect(isSharedRow({ shares: null, sharedUnsupported: true, mine: true })).toBe(true);
    expect(isSharedRow({ shares: null, sharedUnsupported: true, mine: false })).toBe(false);
    expect(isSharedRow({ shares: null, sharedUnsupported: false, mine: true })).toBe(false);
  });

  it("counts held shares from the row, else from EP 5", () => {
    expect(heldShares({ shares: { held: 2, of: 5 } }, sharedShares())).toEqual({ held: 2, of: 5 });
    expect(heldShares({ shares: null }, sharedShares())).toEqual({ held: 1, of: 3 });
    expect(heldShares({ shares: null }, undefined)).toBeNull();
  });

  it("finds the confirmed registration session of the masternode, preferring EP 5's id", () => {
    const s = confirmedCoordinator();
    expect(registrationSession([s], SH_MN, s.id)).toBe(s);
    expect(registrationSession([s], SH_MN, null)).toBe(s);
    expect(registrationSession([s], "ab".repeat(32))).toBeNull();
    expect(registrationSession([{ ...s, stage: { kind: "sent" } }], SH_MN)).toBeNull();
    expect(registrationSession([{ ...s, kind: "key_change" }], SH_MN)).toBeNull();
    expect(registrationSession(undefined, SH_MN)).toBeNull();
  });

  it("labels the legend from the session's rows, else by number, with each share's part of the collateral", () => {
    const withSession = shareLegend(sharedShares(), confirmedCoordinator());
    expect(withSession.map((l) => [l.label, l.amount / COIN, l.mine, percentText(l.percent)])).toEqual([
      ["Alice", 400, true, "40"],
      ["Bob", 300, false, "30"],
      ["Carol", 300, false, "30"],
    ]);
    expect(shareLegend(sharedShares(), null).map((l) => l.label)).toEqual(["Share 1", "Share 2", "Share 3"]);
    expect(percentText(100 / 3)).toBe("33.33");
  });

  it("moves the payout address only on a confirmed update, never on an attempt", () => {
    const at = alice().reward.atRegistration;
    expect(payoutAddress(alice())).toBe(at);
    expect(payoutAddress(alice({ latestAttempt: { operation: "op", address: "yAttempt", state: "sent", sentAt: 1 } }))).toBe(at);
    expect(payoutAddress(alice({ latestConfirmedLocal: { operation: "op", address: "yChanged", confirmedHeight: 5 } }))).toBe("yChanged");
  });

  it("words each attempt state (§2.4.1)", () => {
    const of = (state: "signed" | "not_sent" | "sent" | "conflicted", sentAt: number | null = 7) => attemptLine(alice({ latestAttempt: { operation: "op", address: "yQa7Fixture", state, sentAt } }), short, date);
    expect(attemptLine(alice(), short, date)).toBeNull();
    expect(of("signed")).toBe("A change to yQa7… wasn't sent.");
    expect(of("not_sent")).toBe("A change to yQa7… wasn't sent.");
    expect(of("sent")).toBe("A change to yQa7… was sent t7, not confirmed yet.");
    expect(of("sent", null)).toBe("A change to yQa7… was sent, not confirmed yet.");
    expect(of("conflicted")).toBe("A change to yQa7… didn't go through: a coin it used was spent elsewhere.");
  });

  it("ends the early period at the registration plus the period, or not at all", () => {
    expect(earlyEnd(sharedShares())).toBe(SH_REGISTERED + 105_120);
    expect(earlyEnd(sharedShares({ earlyPeriodBlocks: 0 }))).toBeNull();
  });

  it("replaces the actions with one line per eligibility (§2.4.2)", () => {
    expect(eligibilityLine("ok")).toBeNull();
    expect(eligibilityLine("upgrade_unknown")).toBe("Available once the masternode list has synced.");
    expect(eligibilityLine("list_unavailable")).toBe("The masternode list is still loading. Available once it has synced.");
    expect(eligibilityLine("too_early")).toBe("Available once the masternode list includes the registration.");
    expect(eligibilityLine("delisted")).toBe("This masternode has left the list, so there's nothing to dissolve or change.");
  });

  it("shows only the variants a standby holds, validity from EP 5's full_valid_now", () => {
    const pair = standbyPair(1_319_400);
    expect(standbyLines(pair, false, dash)).toEqual([
      { variant: "full", text: "Full refund: valid from block 1,319,400" },
      { variant: "immediate", text: "Immediate: valid now, you pay the 20 Dash penalty" },
    ]);
    expect(standbyLines(pair, true, dash)).toEqual([{ variant: "full", text: "Full refund: valid now" }]);
    expect(standbyLines({ ...pair, full: false }, true, dash).map((l) => l.variant)).toEqual(["immediate"]);
    expect(standbyLines({ ...pair, immediate: false }, false, dash).map((l) => l.variant)).toEqual(["full"]);
    expect(standbyLines({ ...pair, full: false, penalty: null }, false, dash)).toEqual([{ variant: "immediate", text: "Immediate: valid now, you pay the early-exit penalty" }]);
    expect(standbySource(pair, date)).toBe(`Saved on this computer on t${pair.createdAt}`);
    expect(standbySource({ ...pair, source: "imported" }, date)).toBe("Imported");
  });
});
