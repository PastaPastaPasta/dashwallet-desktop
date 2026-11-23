// In progress, the operations' badge and the Auto sidebar rule (operations.ts, MG-10 §1.10, §3.6).

import { describe, expect, it } from "vitest";
import type { MasternodeOverview } from "../api/generated/MasternodeOverview";
import type { ProTxOperation } from "../api/generated/ProTxOperation";
import type { ProTxOperationStateName } from "../api/generated/ProTxOperationStateName";
import { newMasternodesWorld } from "../fixture/masternodes";
import { t } from "../i18n";
import { autoShows, conflicted, inProgress, lastErrorText, masternodesBadge, opRow, opsBadge, resendCap, resendConfirmText } from "./operations";

const I = "masternodes.inProgress";
const op = (id: string, state: ProTxOperationStateName, patch: Partial<ProTxOperation> = {}): ProTxOperation => ({
  id,
  kind: "registration",
  proTxHash: null,
  txid: "aa".repeat(32),
  state,
  signRequest: null,
  fee: 453,
  debit: 453,
  overdue: false,
  createdAt: 1,
  signedAt: null,
  sentAt: null,
  lastError: null,
  terms: {
    masternodeType: "regular",
    version: 2,
    service: "203.0.113.7:19999",
    ownerAddress: "yO",
    ownerIndex: 0,
    votingAddress: "yV",
    votingIndex: 0,
    operatorPublicKey: "8".repeat(96),
    operatorIndex: null,
    operatorReward: 0,
    payoutAddress: "yP",
    collateral: "fund_new",
    collateralOutpoint: null,
    collateralAddress: null,
    platform: null,
    platformNodeIndex: null,
  },
  ...patch,
});
const every = [
  op("ns", { kind: "needs_signature" }),
  op("is", { kind: "inputs_spent" }),
  op("sg", { kind: "signed" }),
  op("nt", { kind: "not_sent" }, { lastError: "send.no_peers" }),
  op("st", { kind: "sent" }),
  op("cf", { kind: "confirmed", height: 7 }),
  op("cx", { kind: "conflicted" }),
];
const overview = (patch: Partial<MasternodeOverview> = {}): MasternodeOverview => ({
  list: { synced: true, height: 1 },
  masternodes: [],
  collateralCoins: [],
  payouts30d: 0,
  unattributed30d: 0,
  payoutsIncomplete: false,
  ...patch,
});

describe("In progress (§1.10)", () => {
  it("lists every unsettled operation and shows conflicted ones apart", () => {
    expect(inProgress(every).map((o) => o.id)).toEqual(["ns", "is", "sg", "nt", "st"]);
    expect(conflicted(every).map((o) => o.id)).toEqual(["cx"]);
  });

  it("gives each state its row and actions", () => {
    expect(opRow(every[0])).toMatchObject({ status: t(`${I}.status.needsSignature`), primary: "continue", more: ["discard"] });
    expect(opRow(every[1])).toMatchObject({ status: t(`${I}.status.inputsSpent`), primary: "refresh", more: ["discard"] });
    expect(opRow(every[2])).toMatchObject({ status: t(`${I}.status.notSentPlain`), primary: "resend", more: ["discard"] });
    expect(opRow(every[3]).status).toBe(t(`${I}.status.notSent`, { reason: lastErrorText("send.no_peers")! }));
    expect(opRow(every[4])).toMatchObject({ primary: "resend", more: [], overdueLine: null, activityTxid: null });
    expect(opRow(every[6])).toMatchObject({ primary: null, more: [] });
  });

  it("adds the D49 line and Open in Activity once a fee-input row is overdue, and the no-lock line for a dissolve row", () => {
    const late = opRow(op("l", { kind: "sent" }, { overdue: true }));
    expect(late.overdueLine).toBe(t(`${I}.overdue`));
    expect(late.activityTxid).toBe("aa".repeat(32));
    expect(late.primary).toBe("resend");
    const dissolve = opRow(op("d", { kind: "sent" }, { overdue: true, kind: "dissolve" }));
    expect(dissolve.overdueLine).toBe(t(`${I}.overdueDissolve`));
    expect(dissolve.activityTxid).toBeNull();
  });

  it("caps Send again at the row's debit, not its fee (EP 2)", () => {
    const dissolve = op("d", { kind: "sent" }, { kind: "dissolve", fee: 1_000, debit: 51_000 });
    expect(resendCap(dissolve)).toBe(51_000);
    expect(resendCap(op("f", { kind: "not_sent" }, { kind: "dissolve", debit: 0 }))).toBe(0);
    expect(resendConfirmText(dissolve, "X")).toBe(t(`${I}.resendSent`, { debit: "X" }));
    expect(resendConfirmText(every[3], "X")).toBe(t(`${I}.resendNotSent`, { debit: "X" }));
  });

  it("names a wire code with Pay's sentence, else the fallback", () => {
    expect(lastErrorText(null)).toBeNull();
    expect(lastErrorText("send.no_peers")).toBe(t("pay.error.noPeers"));
    expect(lastErrorText("send.something_new")).toBe(t(`${I}.unknownError`, { code: "send.something_new" }));
  });
});

describe("the badge (§3.6, S2)", () => {
  it("counts needs_signature, inputs_spent and not_sent, never a wait", () => {
    expect(opsBadge(every)).toBe(3);
    expect(opsBadge(undefined)).toBe(0);
  });

  it("adds banned own masternodes", () => {
    const rows = newMasternodesWorld().rows;
    expect(masternodesBadge(overview({ masternodes: rows }), every)).toBe(1 + 3);
  });
});

describe("Auto (§3.6)", () => {
  const rows = newMasternodesWorld().rows;
  it("shows for a mine or tracked row, a collateral coin, or unsettled work", () => {
    expect(autoShows(overview(), [])).toBe(false);
    expect(autoShows(undefined, undefined)).toBe(false);
    expect(autoShows(overview({ masternodes: rows.filter((r) => r.mine) }), [])).toBe(true);
    expect(autoShows(overview({ masternodes: rows.filter((r) => r.tracked && !r.mine) }), [])).toBe(true);
    expect(autoShows(overview({ collateralCoins: [{ outpoint: "a-0", address: "y", amount: 1, kind: "regular", confirmations: 1, userLocked: false }] }), [])).toBe(true);
    expect(autoShows(overview(), [every[4]])).toBe(true);
    expect(autoShows(overview(), [every[5], every[6]])).toBe(false);
  });
});
