import { describe, expect, it } from "vitest";
import { coordinatorSession, participantSession } from "../fixture/shared";
import { COIN, isIpv4Service, operatorKeyProblem, parseDash, parsePercent, penaltyError, renderSession, S5_SLICE, sessionNeedsAction, shareErrors, stageStep } from "./shared";

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
