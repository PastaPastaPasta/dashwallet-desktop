// In progress (operations.ts, MG-10 §1.10).

import { describe, expect, it } from "vitest";
import type { ProTxOperation } from "../api/generated/ProTxOperation";
import type { ProTxOperationStateName } from "../api/generated/ProTxOperationStateName";
import { t } from "../i18n";
import { conflicted, inProgress, lastErrorText, opRow, proTxSends, resendCap, resendConfirmText, actsOn } from "./operations";
import { registrarUpdateSends } from "./changeKeys";
import { registrationSends } from "./registration";

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
describe("In progress (§1.10)", () => {
  it("lists every unsettled operation and shows conflicted ones apart", () => {
    expect(inProgress(every).map((o) => o.id)).toEqual(["ns", "is", "sg", "nt", "st"]);
    expect(conflicted(every).map((o) => o.id)).toEqual(["cx"]);
  });

  it("gives each state its row and actions", () => {
    expect(opRow(every[0])).toMatchObject({ status: t(`${I}.status.needsSignature`), primary: null, more: ["discard"] });
    expect(opRow(every[1])).toMatchObject({ status: t(`${I}.status.inputsSpent`), primary: null, more: ["discard"] });
    expect(opRow(every[2])).toMatchObject({ status: t(`${I}.status.notSentPlain`), primary: null, more: ["discard"] });
    expect(opRow(every[3]).status).toBe(t(`${I}.status.notSent`, { reason: lastErrorText("send.no_peers")! }));
    expect(opRow(every[4])).toMatchObject({ primary: null, more: [], overdueLine: null, activityTxid: null });
    expect(opRow(every[6])).toMatchObject({ primary: null, more: [] });
  });

  it("acts on a registration or registrar update only off mainnet (D14), and keeps Discard", () => {
    expect(registrationSends({ kind: "mainnet" })).toBe(false);
    expect(registrarUpdateSends({ kind: "mainnet" })).toBe(false);
    for (const kind of ["testnet", "regtest", "devnet"] as const) {
      expect(registrationSends({ kind })).toBe(true);
      expect(registrarUpdateSends({ kind })).toBe(true);
    }
    expect(registrationSends(null)).toBe(false);
    const sent = op("s", { kind: "sent" });
    const main = proTxSends({ kind: "mainnet" });
    const test = proTxSends({ kind: "testnet" });
    expect(actsOn(sent, main)).toBe(false);
    expect(actsOn(sent)).toBe(false);
    expect(actsOn(sent, test)).toBe(true);
    expect(opRow(op("u", { kind: "not_sent" }, { kind: "update_registrar" }), main)).toMatchObject({ primary: null, more: ["discard"] });
    expect(opRow(op("u", { kind: "sent" }, { kind: "update_registrar" }), test).primary).toBe("resend");
    for (const state of [{ kind: "needs_signature" }, { kind: "inputs_spent" }, { kind: "not_sent" }] as const) {
      expect(opRow(op("r", state), main)).toMatchObject({ primary: null, more: ["discard"] });
      expect(opRow(op("r", state), test).primary).not.toBeNull();
    }
    expect(opRow(op("d", { kind: "sent" }, { kind: "dissolve" }), main).primary).toBe("resend");
    expect(opRow(op("d", { kind: "not_sent" }, { kind: "dissolve" }), test).primary).toBe("resend");
  });

  it("adds the D49 line and Open in Activity once a fee-input row is overdue, and the no-lock line for a dissolve row", () => {
    const late = opRow(op("l", { kind: "sent" }, { overdue: true }));
    expect(late.overdueLine).toBe(t(`${I}.overdue`));
    expect(late.activityTxid).toBe("aa".repeat(32));
    expect(late.primary).toBeNull();
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
