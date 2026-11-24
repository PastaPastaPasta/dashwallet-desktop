import { describe, expect, it } from "vitest";
import type { SharedNextName } from "../api/generated/SharedNextName";
import type { SharedSession } from "../api/generated/SharedSession";
import { coordinatorSession, participantSession } from "../fixture/shared";
import { en } from "../i18n/en";
import { flatten } from "../i18n/core";
import { t } from "../i18n";
import { SHARED_ABANDON_FEE } from "../api/generated/commands";
import { abandonNeedsOwnFirst, canAbandon, COIN, isIpv4Service, warningText, operatorKeyProblem, parseDash, parsePercent, penaltyError, renderSession, S5_SLICE, sessionNeedsAction, shareErrors, signedOutText, stageStep, unreadableInProgress } from "./shared";

const shares = (...amounts: number[]) => amounts.map((a, i) => ({ label: `P${i}`, amount: a * COIN }));

describe("shared masternode setup checks", () => {
  it("takes 2 to 8 shares of at least 100 Dash summing to 1,000", () => {
    expect(shareErrors(shares(500, 500))).toEqual([]);
    expect(shareErrors(shares(1000))).toEqual(["count"]);
    expect(shareErrors(shares(950, 50))).toEqual(["min"]);
    expect(shareErrors(shares(500, 400))).toEqual(["sum"]);
    expect(shareErrors([...shares(500), { label: " ", amount: 500 * COIN }])).toEqual(["label"]);
    expect(shareErrors([{ label: "A", amount: 500 * COIN }, { label: "A ", amount: 500 * COIN }])).toEqual(["duplicate"]);
  });

  it("keeps the penalty under the smallest share, and 0 without an early period", () => {
    expect(penaltyError(shares(700, 300), 105_120, 299 * COIN)).toBeNull();
    expect(penaltyError(shares(700, 300), 105_120, 300 * COIN)).toBe("penalty");
    expect(penaltyError(shares(700, 300), 0, 0)).toBeNull();
    expect(penaltyError(shares(700, 300), 0, 1)).toBe("penaltyNoPeriod");
  });

  it("parses Dash, percentages, operator keys and IPv4 services", () => {
    expect(parseDash("12.5")).toBe(12.5 * COIN);
    expect(parseDash("0,001")).toBe(100_000);
    expect(parseDash("1e3")).toBeNull();
    expect(parsePercent("2.5")).toBe(250);
    expect(parsePercent("100.01")).toBeNull();
    expect(operatorKeyProblem("ab".repeat(48))).toBeNull();
    expect(operatorKeyProblem("ab".repeat(32))).toBe("secret");
    expect(operatorKeyProblem("xyz")).toBe("invalid");
    expect(isIpv4Service("203.0.113.7:19999")).toBe(true);
    expect(isIpv4Service("203.0.113.07:19999")).toBe(false);
    expect(isIpv4Service("203.0.113.7")).toBe(false);
  });
});

describe("the session's render under this slice", () => {
  it("offers no signing or sending while they are off", () => {
    const combined = participantSession("combined");
    const v = renderSession(combined, S5_SLICE);
    expect(v.row).toBe("sign");
    expect(v.primary).toBeNull();
    expect(sessionNeedsAction(combined)).toBe(false);
    expect(renderSession(combined, { sign: true, send: true }).primary).toBe("sign");
  });

  it("never renders a signing or sending step, whatever the stage, role or next", () => {
    const stages: SharedSession["stage"][] = [{ kind: "invited" }, { kind: "draft" }, { kind: "signing" }, { kind: "combined" }, { kind: "funding_signed" }, { kind: "sent" }, { kind: "confirmed", height: 1 }, { kind: "conflicted" }, { kind: "stale" }, { kind: "dead", reason: "abandoned" }];
    const nexts: SharedNextName[] = ["join", "copy", "wait", "lock", "approve", "sign", "send", "create_standby", "abandon", "nothing"];
    for (const role of ["coordinator", "participant"] as const)
      for (const stage of stages)
        for (const next of nexts) {
          const v = renderSession({ ...participantSession("combined"), role, stage, next }, S5_SLICE);
          const at = `${role} ${stage.kind} ${next}`;
          expect(["sendSigned", "waitingSigned", "send"], at).not.toContain(v.row);
          expect(["sign", "send"], at).not.toContain(v.primary);
        }
  });

  it("knows every fixture session and maps each to a stage", () => {
    const sessions = [coordinatorSession(), coordinatorSession(["Bob"]), coordinatorSession(["Bob", "Carol"]), ...(["invited", "draft", "signing", "combined", "funding_signed", "sent", "dead", "confirmed"] as const).map((k) => participantSession(k))];
    for (const s of sessions) {
      expect(renderSession(s).known, `${s.role} ${s.stage.kind} ${s.next}`).toBe(true);
      expect(stageStep(s)).toBeGreaterThanOrEqual(1);
    }
    expect(renderSession(coordinatorSession(["Bob", "Carol"])).primary).toBe("lock");
    expect(renderSession(participantSession("invited")).primary).toBe("join");
    expect(renderSession(participantSession("signing")).primary).toBe("approve");
  });
});

describe("a session whose funding signature may be out", () => {
  it("never says nothing has left, and never advises Discard", () => {
    const out = { ...participantSession("funding_signed"), next: "copy" as const, discardable: false };
    expect(renderSession(out).row).toBe("signedOut");
    expect(renderSession({ ...out, discardable: true }).row).toBe("signLater");
    const w = { kind: "unvalued_inputs" as const, shares: [0], count: 1 };
    expect(warningText(w, out)).not.toContain("Discard");
    expect(warningText(w, out)).toContain("abandon");
    expect(warningText(w, { ...out, discardable: true })).toContain("Discard");
    expect(warningText(w, { ...out, abandonable: false }), "no Abandon to point at").toBeNull();
    expect(warningText(w, { ...participantSession("confirmed"), discardable: false })).toBeNull();
  });
});

describe("Abandon", () => {
  const catalog = flatten(en);
  const label = t("masternodes.shared.session.abandon");

  it("names the button by its own label in the signed-out line", () => {
    expect(String(catalog.get("masternodes.shared.row.signedOut")).match(/\{abandon\}/g)).toHaveLength(1);
    expect(label).toBe("Abandon");
    expect(signedOutText()).toContain(label);
    expect(signedOutText()).not.toContain("{abandon}");
  });

  it("is offered by the engine's rule: may be out, not settled, a reserved coin still held", () => {
    const out = participantSession("funding_signed");
    expect(canAbandon(out)).toBe(true);
    expect(canAbandon(participantSession("sent"))).toBe(true);
    expect(canAbandon({ ...out, discardable: true })).toBe(false);
    expect(canAbandon({ ...out, abandonable: false })).toBe(false);
    expect(canAbandon({ ...out, reserved: [] })).toBe(false);
    for (const stage of [{ kind: "confirmed", height: 1 }, { kind: "conflicted" }, { kind: "dead", reason: "inputs_spent" }] as const) expect(canAbandon({ ...out, stage })).toBe(false);
    expect(canAbandon({ ...out, kind: "dissolve_together" })).toBe(false);
    for (const k of ["invited", "draft", "signing", "combined", "confirmed", "dead"] as const) expect(canAbandon(participantSession(k)), k).toBe(false);
  });

  it("reads a refusal for want of a coin as the wallet's own transaction coming first", () => {
    const refusal = (stage: string) => ({ code: "masternodes.shared_stage", message: "m", data: { kind: "sharedStage", stage } });
    expect(abandonNeedsOwnFirst(refusal("sent"))).toBe(true);
    expect(abandonNeedsOwnFirst(refusal("funding_signed"))).toBe(true);
    expect(abandonNeedsOwnFirst(refusal("conflicted"))).toBe(false);
    expect(abandonNeedsOwnFirst({ code: "send.grant_exceeded", message: "m", data: null })).toBe(false);
    expect(abandonNeedsOwnFirst(new Error("x"))).toBe(false);
  });

  it("caps its grant at the engine's exact fee", () => {
    expect(SHARED_ABANDON_FEE).toBe(193);
  });
});

describe("unreadable sessions in progress", () => {
  const row = { id: "a".repeat(64), code: "AAAAAA", kind: "registration" as const, role: "participant" as const, discardable: false, abandonable: false, reserved: ["x-0"], updatedAt: 1 };
  it("lists one it may discard or one still holding coins, and drops one whose locks were released", () => {
    expect(unreadableInProgress([row])).toHaveLength(1);
    expect(unreadableInProgress([{ ...row, discardable: true, reserved: [] }])).toHaveLength(1);
    expect(unreadableInProgress([{ ...row, reserved: [] }])).toEqual([]);
    expect(unreadableInProgress(undefined)).toEqual([]);
  });
});
