// Shared-masternode maintenance (MG-10 §2.5–§2.8, S6): the dissolve-together and key-change rows,
// the final stages (F4), the grant caps, and the mainnet ruling's slice and copy.

import { describe, expect, it } from "vitest";
import type { SharedSession } from "../api/generated/SharedSession";
import { keyChangeSession, participantSession, sharedShares, standbyPair, togetherSession } from "../fixture/shared";
import { t } from "../i18n";
import {
  approveMaintenanceCap,
  canDiscard,
  COIN,
  keyChangeLines,
  renderSession,
  S5_SLICE,
  sessionNeedsAction,
  sessionsInProgress,
  sharedSendsOn,
  sliceFor,
  standbyCreateCap,
  standbyHolderPenalty,
  standbyMissing,
  standbySend,
  togetherFee,
  togetherReturn,
} from "./shared";

const MAINNET = sliceFor({ kind: "mainnet" });
/** S6 where sends are on. */
const S6_SLICE = sliceFor({ kind: "testnet" });

describe("dissolve together under S6", () => {
  it("approves, copies the approval, then sends the combined dissolution", () => {
    expect(renderSession(togetherSession("participant", "signing"), S6_SLICE)).toMatchObject({ row: "togetherApprove", primary: "approveMaintenance", known: true });
    expect(renderSession(togetherSession("coordinator", "signing"), S6_SLICE)).toMatchObject({ row: "togetherSendApproval", primary: null, copy: true });
    expect(renderSession(togetherSession("participant", "combined"), S6_SLICE)).toMatchObject({ row: "togetherSend", primary: "sendMaintenance" });
    expect(renderSession({ ...togetherSession("participant", "combined"), stage: { kind: "confirmed", height: 9 }, next: "nothing" }, S6_SLICE).row).toBe("dissolved");
  });

  it("on mainnet keeps the approval and drops the send for the note", () => {
    expect(renderSession(togetherSession("participant", "signing"), MAINNET).primary).toBe("approveMaintenance");
    expect(renderSession(togetherSession("participant", "combined"), MAINNET)).toMatchObject({ row: "togetherSendOff", primary: null, copy: true, known: true });
    expect(sessionNeedsAction(togetherSession("participant", "combined"), MAINNET)).toBe(false);
  });

  it("stays unknown, with no action, before S6", () => {
    for (const s of [togetherSession("participant", "signing"), keyChangeSession("participant", "signing")]) {
      expect(renderSession(s, S5_SLICE)).toMatchObject({ row: "unknown", primary: null, known: false });
      expect(sessionNeedsAction(s, S5_SLICE)).toBe(false);
      expect(sessionsInProgress([s], S5_SLICE)).toEqual([]);
    }
  });

  it("caps the approval at the accepted actor fee, else 0; returns this wallet's share less its fee", () => {
    const s = togetherSession("participant", "signing", { actorFee: 1_000 });
    expect(approveMaintenanceCap(s, true)).toBe(1_000);
    expect(approveMaintenanceCap(s, false)).toBe(0);
    expect(approveMaintenanceCap(keyChangeSession("participant", "signing", { actorFee: 1_000 }), true)).toBe(0);
    expect(togetherReturn(s)).toBe(400 * COIN - 1_000);
    expect(togetherReturn(togetherSession("participant", "signing"))).toBe(400 * COIN);
    expect(togetherFee(3)).toBe(1_000);
    expect(togetherFee(8)).toBe(1_180);
  });
});

describe("key change under S6", () => {
  it("on mainnet, where the engine refuses it, offers no approval and asks for nothing", () => {
    const s = keyChangeSession("participant", "signing");
    expect(renderSession(s, MAINNET)).toMatchObject({ row: "keyApproveOff", primary: null, known: true });
    expect(sessionNeedsAction(s, MAINNET)).toBe(false);
    expect(sessionNeedsAction(s, S6_SLICE)).toBe(true);
  });

  it("approves and copies where the engine allows it, and never sends on any network", () => {
    expect(renderSession(keyChangeSession("participant", "signing"), S6_SLICE)).toMatchObject({ row: "keyApprove", primary: "approveMaintenance" });
    expect(renderSession(keyChangeSession("coordinator", "signing"), S6_SLICE)).toMatchObject({ row: "keyWaiting", copy: true });
    expect(renderSession(keyChangeSession("participant", "combined"), S6_SLICE)).toMatchObject({ row: "keySendSignatures", copy: true });
    expect(renderSession(keyChangeSession("coordinator", "combined"), S6_SLICE)).toMatchObject({ row: "keySendLater", primary: null, known: true });
    expect(renderSession(keyChangeSession("coordinator", "stale"), S6_SLICE).row).toBe("keyStale");
    expect(renderSession(keyChangeSession("participant", "stale"), S6_SLICE).row).toBe("keyStaleOther");
    const stages: SharedSession["stage"][] = [{ kind: "signing" }, { kind: "combined" }, { kind: "sent" }, { kind: "stale" }, { kind: "confirmed", height: 1 }, { kind: "conflicted" }, { kind: "dead", reason: "abandoned" }];
    for (const slice of [S6_SLICE, MAINNET])
      for (const role of ["coordinator", "participant"] as const)
        for (const stage of stages)
          for (const next of ["approve", "copy", "send", "wait", "abandon", "nothing"] as const) {
            const v = renderSession({ ...keyChangeSession(role, "combined"), stage, next }, slice);
            expect(v.primary, `${role} ${stage.kind} ${next}`).not.toBe("sendMaintenance");
          }
  });

  it("reads the keys current → requested", () => {
    const [operator, voting] = keyChangeLines(keyChangeSession("participant", "signing"));
    expect(operator).toMatchObject({ field: "operator", changes: false });
    expect(voting).toMatchObject({ field: "voting", changes: true });
    expect(voting!.from).not.toBe(voting!.to);
  });
});

describe("registrations keep S5b's rule under S6", () => {
  it("sign and send where sends are on, and not on mainnet", () => {
    for (const stage of ["combined", "funding_signed"] as const) {
      expect(renderSession(participantSession(stage, { role: "coordinator", next: "send" }), S6_SLICE).primary).toBe("send");
      expect(renderSession(participantSession(stage, { role: "coordinator", next: "send" }), MAINNET).primary).toBeNull();
    }
    expect(renderSession(participantSession("combined", { next: "sign" }), S6_SLICE).primary).toBe("sign");
    expect(renderSession(participantSession("combined", { next: "sign" }), MAINNET).primary).toBeNull();
  });
});

describe("final stages (F4)", () => {
  const finals: SharedSession["stage"][] = [{ kind: "confirmed", height: 1 }, { kind: "conflicted" }, { kind: "dead", reason: "abandoned" }];
  for (const stage of finals)
    it(`${stage.kind}: not in progress and no Discard once the engine reads it not discardable`, () => {
      const sessions = [
        ...(stage.kind === "dead" ? [] : [{ ...participantSession("signing"), stage, discardable: false }]),
        { ...togetherSession("participant", "signing"), stage, discardable: false },
        { ...keyChangeSession("coordinator", "combined"), stage, discardable: false },
      ];
      expect(sessionsInProgress(sessions, S6_SLICE)).toEqual([]);
      for (const s of sessions) expect(canDiscard(s), s.kind).toBe(false);
    });

  it("a dead session that still holds coins stays listed with Discard until it is discarded", () => {
    const s = { ...participantSession("signing"), stage: { kind: "dead", reason: "inputs_spent" } as SharedSession["stage"], discardable: true };
    expect(sessionsInProgress([s], S6_SLICE)).toEqual([s]);
    expect(canDiscard(s)).toBe(true);
  });

  it("an open discardable session is listed and offers Discard", () => {
    const s = keyChangeSession("coordinator", "combined");
    expect(sessionsInProgress([s], S6_SLICE)).toEqual([s]);
    expect(canDiscard(s)).toBe(true);
    expect(canDiscard({ ...s, discardable: false })).toBe(false);
  });
});

describe("standby caps (§3.4, D14)", () => {
  it("caps create at the penalty and fee for each of this wallet's shares", () => {
    expect(standbyCreateCap(sharedShares({}), 1_000)).toBe(20 * COIN + 1_000);
  });

  it("offers one send: full once valid, else immediate with a known penalty, else none", () => {
    const pair = standbyPair();
    expect(standbySend(pair, false, true)).toMatchObject({ variant: "immediate", label: "sendImmediate", cap: 1_000 + 20 * COIN });
    expect(standbySend(pair, true, true)).toMatchObject({ variant: "full", label: "sendFull", cap: 1_000 });
    expect(standbySend(pair, true, false)).toMatchObject({ variant: "full", cap: 0 });
    expect(standbySend({ ...pair, full: false }, true, true)).toMatchObject({ variant: "immediate", label: "sendIt", stillPenalty: true });
    expect(standbySend({ ...pair, penalty: null }, false, true)).toBeNull();
    expect(standbySend({ ...pair, immediate: false }, false, true)).toBeNull();
  });

  it("offers Create for an owned share with no variant the engine would take now", () => {
    const pair = standbyPair();
    const mine = [{ index: pair.shareIndex }];
    expect(standbyMissing(mine, [], false)).toBe(true);
    expect(standbyMissing(mine, [pair], false)).toBe(false);
    expect(standbyMissing(mine, [{ ...pair, immediate: false }], false)).toBe(true);
    expect(standbyMissing(mine, [{ ...pair, immediate: false }], true)).toBe(false);
    expect(standbyMissing(mine, [{ ...pair, shareIndex: pair.shareIndex + 1 }], true)).toBe(true);
    expect(standbyMissing([], [], false)).toBe(false);
  });

  it("names the penalty a holder of the saved file can charge, after the early period too", () => {
    const pair = standbyPair();
    expect(standbyHolderPenalty(pair)).toBe(20 * COIN);
    expect(standbyHolderPenalty({ ...pair, immediate: false })).toBe(0);
    expect(standbyHolderPenalty({ ...pair, penalty: null })).toBe(0);
    expect(standbyHolderPenalty({ ...pair, penalty: 0 })).toBe(0);
  });
});

describe("the mainnet ruling", () => {
  it("turns maintenance sends off on mainnet only", () => {
    expect(sharedSendsOn({ kind: "mainnet" })).toBe(false);
    for (const kind of ["testnet", "regtest", "devnet"] as const) {
      expect(sharedSendsOn({ kind })).toBe(true);
      expect(sliceFor({ kind })).toMatchObject({ maintenance: true, maintenanceSend: true, keyChange: true, sign: true, send: true });
    }
    expect(MAINNET).toMatchObject({ maintenance: true, maintenanceSend: false, keyChange: false, sign: false, send: false });
  });

  it("names the export button verbatim in the mainnet Dissolve line", () => {
    const label = t("masternodes.shared.detail.standby.saveCopy");
    expect(label).toBe("Save a copy…");
    expect(t("masternodes.shared.mainnetDissolveNote")).toContain(label);
  });
});
