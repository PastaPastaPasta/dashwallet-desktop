// Change keys or payout's model (MG-10 §1.7): Q4's payout preselection, the request it sends and
// the refusals it names.

import { describe, expect, it } from "vitest";
import type { MasternodeDetail } from "../api/generated/MasternodeDetail";
import type { ProTxOperation } from "../api/generated/ProTxOperation";
import type { UpdateRegistrarPreview } from "../api/generated/UpdateRegistrarPreview";
import { buildRequest, changeKeysErrorText, changesNothing, fieldErrors, initialForm, keepsRecordedPayout, pinToPreview, reviewLines, signerLine, unsentUpdate } from "./changeKeys";

const HASH = "4e".repeat(32);
const RECORDED = "yRecordedPayoutAddressMn7qR2vWx9Kf";
const KEY = "ab".repeat(48);

const detail = (payoutAddress: string | null) => ({ registration: payoutAddress === null ? null : { payoutAddress } }) as unknown as MasternodeDetail;

describe("Change keys or payout", () => {
  it("keeps both keys and preselects the recorded payout only when the wallet records one (Q4)", () => {
    const f = initialForm(detail(RECORDED));
    expect(f).toMatchObject({ operator: "keep", voting: "keep", payout: "recorded" });
    expect(buildRequest(HASH, f, RECORDED)).toEqual({ proTxHash: HASH, operator: { kind: "keep" }, voting: { kind: "keep" }, payout: { kind: "address", address: RECORDED } });
    const none = initialForm(detail(null));
    expect(none.payout).toBeNull();
    expect(buildRequest(HASH, none, null)).toBeNull();
  });

  it("sends the keychain's next keys, a given operator public key and another address", () => {
    const base = initialForm(detail(null));
    expect(buildRequest(HASH, { ...base, operator: "keychain", voting: "keychain", payout: "new" }, null)).toEqual({
      proTxHash: HASH,
      operator: { kind: "next_unused" },
      voting: { kind: "next_unused" },
      payout: { kind: "new_address" },
    });
    const given = { ...base, operator: "given" as const, operatorKey: ` ${KEY.toUpperCase()} `, voting: "address" as const, votingAddress: "yVotingAddressMn7qR2vWx9KfT4Mn7qR2v", payout: "address" as const, payoutAddress: "yPayoutAddressMn7qR2vWx9KfT4Mn7qR2v" };
    expect(buildRequest(HASH, given, null)).toEqual({
      proTxHash: HASH,
      operator: { kind: "given", publicKey: KEY },
      voting: { kind: "address", address: "yVotingAddressMn7qR2vWx9KfT4Mn7qR2v" },
      payout: { kind: "address", address: "yPayoutAddressMn7qR2vWx9KfT4Mn7qR2v" },
    });
  });

  it("holds the request while a field is empty or wrong, and names only the wrong ones", () => {
    const base = { ...initialForm(detail(RECORDED)) };
    expect(buildRequest(HASH, { ...base, operator: "given" }, RECORDED)).toBeNull();
    expect(fieldErrors({ ...base, operator: "given" })).toEqual({});
    const bad = { ...base, operator: "given" as const, operatorKey: "abc", voting: "address" as const, votingAddress: "0OIl", payout: "address" as const, payoutAddress: "nope!" };
    expect(Object.keys(fieldErrors(bad)).sort()).toEqual(["operatorKey", "payoutAddress", "votingAddress"]);
    expect(buildRequest(HASH, bad, RECORDED)).toBeNull();
  });

  it("reads the preview: what changes, the signer, and a change of nothing", () => {
    const p: UpdateRegistrarPreview = {
      terms: { operatorPublicKey: KEY, votingAddress: "yVoting", payoutAddress: "" } as UpdateRegistrarPreview["terms"],
      version: 2,
      fee: 452,
      totalDebit: 452,
      ownerKey: { kind: "attached" },
      operatorChanges: true,
      votingChanges: false,
      payoutChanges: null,
      poseBans: true,
    };
    const lines = reviewLines(p);
    expect(lines.map((l) => [l.id, l.changes])).toEqual([
      ["operator", true],
      ["voting", false],
      ["payout", true],
    ]);
    expect(lines[2].value).toBe("a new address in this wallet");
    expect(lines[2].status).toBe("Set");
    expect(signerLine(p)).toBe("Signed with the owner key you added.");
    expect(changesNothing(p)).toBe(false);
    expect(changesNothing({ ...p, operatorChanges: false, payoutChanges: false })).toBe(true);
  });

  it("names the payee-reuse and duplicate-key refusals, else the masternodes sentence", () => {
    const refused = (reason: string) => ({ code: "masternodes.protx_refused", message: reason, data: { kind: "reason", reason } });
    expect(changeKeysErrorText(refused("bad-protx-payee-reuse"))).toMatch(/owner or voting key/);
    expect(changeKeysErrorText(refused("protx-dup"))).toMatch(/already uses that operator key/);
    expect(changeKeysErrorText(refused("bad-protx-dup-key"))).toMatch(/Another masternode already uses/);
    expect(changeKeysErrorText({ code: "send.grant_exceeded", message: "x", data: { kind: "spendCap", maxDuffs: 1, outflow: 2 } })).toMatch(/fee changed/);
    expect(changeKeysErrorText({ code: "masternodes.update_pending", message: "x", data: null })).toMatch(/hasn't reached the masternode list/);
  });

  it("tells a sent pending change, which only an abandon in Activity frees, from one never sent (#438)", () => {
    const pending = { code: "masternodes.update_pending", message: "x", data: { kind: "operation", operation: "u1" } };
    const op = (kind: ProTxOperation["state"]["kind"]) => [{ id: "u1", state: { kind } } as ProTxOperation];
    const SENT =
      "An earlier change to this masternode was sent but hasn't reached the masternode list yet. Wait for it to confirm. If it never will, because it was replaced or dropped from the network, abandon it in Activity. That frees its coins, and you can make a new change. Don't abandon a change that may still be on the network.";
    const NEVER_SENT = "An earlier change to this masternode hasn't reached the masternode list yet. Wait for it, or discard it in In progress if it was never sent.";
    expect(changeKeysErrorText(pending, op("sent"))).toBe(SENT);
    expect(changeKeysErrorText(pending, op("not_sent"))).toBe(NEVER_SENT);
    expect(changeKeysErrorText(pending, op("signed"))).toBe(NEVER_SENT);
    // Not read yet, or another operation: the line that claims nothing about a send.
    expect(changeKeysErrorText(pending, undefined)).toBe(NEVER_SENT);
    expect(changeKeysErrorText({ ...pending, data: { kind: "operation", operation: "other" } }, op("sent"))).toBe(NEVER_SENT);
  });

  it("keeps the recorded payout only when the preview reads it as no change (Q4: false, true, null)", () => {
    const p = (payoutChanges: boolean | null) => ({ payoutChanges }) as UpdateRegistrarPreview;
    expect(keepsRecordedPayout(p(false))).toBe(true);
    expect(keepsRecordedPayout(p(true))).toBe(false);
    expect(keepsRecordedPayout(p(null))).toBe(false);
  });

  it("signs the reviewed keychain indexes, not a fresh next-unused pick", () => {
    const r = buildRequest(HASH, { ...initialForm(detail(null)), operator: "keychain", voting: "keychain", payout: "new" }, null)!;
    const p = { terms: { operatorIndex: 3, votingIndex: 5 } } as UpdateRegistrarPreview;
    expect(pinToPreview(r, p)).toEqual({ ...r, operator: { kind: "keychain", index: 3 }, voting: { kind: "keychain", index: 5 } });
    const kept = buildRequest(HASH, initialForm(detail(RECORDED)), RECORDED)!;
    expect(pinToPreview(kept, { terms: { operatorIndex: null, votingIndex: null } } as UpdateRegistrarPreview)).toEqual(kept);
  });

  it("finds this masternode's update that didn't go out", () => {
    const op = (state: ProTxOperation["state"], kind: ProTxOperation["kind"] = "update_registrar", proTxHash = HASH) => ({ id: state.kind, kind, proTxHash, state }) as ProTxOperation;
    expect(unsentUpdate([op({ kind: "sent" }), op({ kind: "not_sent" })], HASH)?.id).toBe("not_sent");
    expect(unsentUpdate([op({ kind: "signed" }, "registration")], HASH)).toBeNull();
    expect(unsentUpdate([op({ kind: "not_sent" }, "update_registrar", "ab".repeat(32))], HASH)).toBeNull();
    expect(unsentUpdate(undefined, HASH)).toBeNull();
  });
});
