import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import type { SharedNextName } from "../api/generated/SharedNextName";
import type { SharedSession } from "../api/generated/SharedSession";
import { coordinatorSession, participantSession } from "../fixture/shared";
import { en } from "../i18n/en";
import { flatten } from "../i18n/core";
import { t } from "../i18n";
import {
  abandonNeedsOwnFirst,
  canAbandon,
  COIN,
  debitParts,
  grantCap,
  handoffLine,
  isIpv4Service,
  mainnetHoldsBack,
  NO_SENDS,
  operatorKeyProblem,
  parseDash,
  parsePercent,
  penaltyError,
  REFUSED_HANDOFF,
  renderSession,
  reservationNote,
  S5_SLICE,
  sendOffer,
  sessionNeedsAction,
  setupOff,
  setupSteps,
  sharedErrorText,
  sharedSendsOn,
  shareErrors,
  signedOutText,
  sliceFor,
  stageStep,
  unreadableInProgress,
  warningText,
  feeRowValue,
  spendBackAction,
  spendBackLine,
  wayOutShown,
} from "./shared";

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

const NETWORKS = { mainnet: { kind: "mainnet" }, testnet: { kind: "testnet" }, regtest: { kind: "regtest" }, devnet: { kind: "devnet" } } as const;
const coordinator = (stage: SharedSession["stage"], next: SharedNextName, patch: Partial<SharedSession> = {}) => coordinatorSession(["Bob", "Carol"], { stage, next, ...patch });

describe("the session's render per network", () => {
  it("signs and sends on testnet, regtest and devnet, and not on mainnet", () => {
    expect(sliceFor(NETWORKS.mainnet)).toEqual(NO_SENDS);
    expect(sharedSendsOn(NETWORKS.mainnet)).toBe(false);
    for (const n of [NETWORKS.testnet, NETWORKS.regtest, NETWORKS.devnet]) {
      expect(sliceFor(n), n.kind).toEqual(S5_SLICE);
      expect(sharedSendsOn(n), n.kind).toBe(true);
    }
    expect(S5_SLICE).toEqual({ sign: true, send: true });
  });

  it("renders each sign and send row by role and stage where sends are on", () => {
    const cases: [SharedSession, string, string | null, boolean][] = [
      [participantSession("combined"), "sign", "sign", false],
      [participantSession("funding_signed"), "sendSigned", null, true],
      [participantSession("sent"), "sent", null, false],
      [coordinator({ kind: "combined" }, "copy"), "waitingSigned", null, true],
      [coordinator({ kind: "funding_signed" }, "copy"), "waitingSigned", null, true],
      [coordinator({ kind: "combined" }, "send"), "send", "send", false],
      [coordinator({ kind: "funding_signed" }, "send"), "send", "send", false],
      [coordinator({ kind: "sent" }, "wait", { discardable: false }), "sent", "resend", false],
    ];
    for (const [s, row, primary, copy] of cases) {
      const at = `${s.role} ${s.stage.kind} ${s.next}`;
      const v = renderSession(s, S5_SLICE);
      expect(v, at).toEqual({ row, primary, copy, known: true });
    }
  });

  it("says coins are reserved whole on Join, the setup estimate and Keep", () => {
    expect(t("masternodes.shared.row.joinButton", { amount: "300 DASH" })).toBe("Join: reserve coins covering 300 DASH");
    expect(t("masternodes.shared.funding.estimateCoordinator", { amount: "400.001 DASH", share: "400 DASH", fee: "0.001 DASH" })).toBe(
      "Coins covering about 400.001 DASH will be reserved: your 400 DASH share and the 0.001 DASH network fee. Coins are reserved whole, so if a coin is larger than needed, all of it stays unavailable for other payments while the session lasts.",
    );
    expect(t("masternodes.shared.close.text", { debit: "300 DASH" })).toBe(
      "Keeping it leaves the coins covering your 300 DASH reserved, whole, so you can carry on where you left off. Discarding releases them. If the others are still waiting for you, they have to start over.",
    );
  });

  it("turns off setting up and joining on mainnet with one reason, and keeps Discard and Abandon", () => {
    expect(setupOff(NETWORKS.mainnet)).toBe("This version can set up and join shared masternodes on test networks only.");
    for (const n of [NETWORKS.testnet, NETWORKS.regtest, NETWORKS.devnet]) expect(setupOff(n), n.kind).toBeNull();
    const cases: [SharedSession, string[]][] = [
      [participantSession("invited"), ["join"]],
      [coordinatorSession(), ["fee"]],
      [coordinatorSession(["Bob", "Carol"]), ["lock"]],
      [participantSession("signing"), ["approve"]],
      [coordinator({ kind: "signing" }, "copy"), ["unlock"]],
      [participantSession("combined"), []],
      [participantSession("funding_signed"), []],
    ];
    for (const [s, steps] of cases) {
      const at = `${s.role} ${s.stage.kind} ${s.next}`;
      expect(setupSteps(s, sliceFor(NETWORKS.mainnet)), at).toEqual(steps);
      expect(setupSteps(s, sliceFor(NETWORKS.testnet)), at).toEqual(steps);
    }
    expect(coordinatorSession().discardable).toBe(true);
    expect(canAbandon(participantSession("funding_signed"))).toBe(true);
  });

  it("keeps mainnet's rows as before sends: no Sign, Send or Send again, and the note says why", () => {
    const sessions = [participantSession("combined"), coordinator({ kind: "combined" }, "send"), coordinator({ kind: "sent" }, "wait", { discardable: false })];
    for (const s of sessions) {
      const at = `${s.role} ${s.stage.kind}`;
      expect(sendOffer(s, NETWORKS.mainnet, false), at).toBeNull();
      expect(mainnetHoldsBack(s, NETWORKS.mainnet), at).toBe(true);
      expect(sessionNeedsAction(s, sliceFor(NETWORKS.mainnet)), at).toBe(false);
      for (const n of [NETWORKS.testnet, NETWORKS.regtest, NETWORKS.devnet]) {
        expect(sendOffer(s, n, false), `${at} ${n.kind}`).not.toBeNull();
        expect(mainnetHoldsBack(s, n), `${at} ${n.kind}`).toBe(false);
      }
    }
    expect(renderSession(participantSession("combined"), NO_SENDS).row).toBe("signLater");
    expect(renderSession(coordinator({ kind: "combined" }, "send"), NO_SENDS).row).toBe("signLater");
    expect(mainnetHoldsBack(participantSession("signing"), NETWORKS.mainnet)).toBe(false);
  });

  it("offers no funds call on a row the table doesn't list", () => {
    expect(sendOffer({ ...coordinator({ kind: "combined" }, "send"), next: "sign" }, NETWORKS.testnet, false)).toBeNull();
    expect(sendOffer({ ...participantSession("combined"), role: "coordinator" }, NETWORKS.testnet, false)).toBeNull();
  });

  it("offers no Send while a contribution is unsigned (the engine's next is copy)", () => {
    const waiting = coordinator({ kind: "combined" }, "copy");
    expect(sendOffer(waiting, NETWORKS.testnet, false)).toBeNull();
    expect(renderSession(waiting, S5_SLICE).primary).toBeNull();
  });

  it("badges Sign and Send, never Send again", () => {
    expect(sessionNeedsAction(participantSession("combined"), S5_SLICE)).toBe(true);
    expect(sessionNeedsAction(coordinator({ kind: "combined" }, "send"), S5_SLICE)).toBe(true);
    expect(sessionNeedsAction(coordinator({ kind: "sent" }, "wait"), S5_SLICE)).toBe(false);
    expect(sessionNeedsAction(participantSession("combined"))).toBe(false);
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

describe("the grant cap of a sign or send", () => {
  it("is the session's debit exactly, for both roles, with and without dust", () => {
    const sessions = [
      participantSession("combined"),
      participantSession("combined", { debit: 300 * COIN + 4_000 }),
      coordinator({ kind: "combined" }, "send"),
      coordinator({ kind: "combined" }, "send", { debit: 400 * COIN + 100_000 + 4_000 }),
      coordinator({ kind: "sent" }, "wait"),
    ];
    for (const s of sessions) {
      const action = sendOffer(s, NETWORKS.testnet, false)!;
      expect(action, `${s.role} ${s.stage.kind}`).not.toBeNull();
      expect(grantCap(action, s)).toBe(s.debit);
      expect(grantCap(action, s)).toBe(debitParts(s)!.debit);
    }
    expect(grantCap("lock", coordinatorSession(["Bob", "Carol"]))).toBe(0);
    expect(grantCap("approve", participantSession("signing"))).toBe(0);
  });

  it("offers no action without a debit, and none to a watch-only wallet", () => {
    for (const s of [participantSession("combined"), coordinator({ kind: "combined" }, "send"), coordinator({ kind: "sent" }, "wait")]) {
      const at = `${s.role} ${s.stage.kind}`;
      expect(grantCap(sendOffer(s, NETWORKS.testnet, false)!, { ...s, debit: null }), at).toBeNull();
      expect(sendOffer({ ...s, debit: null }, NETWORKS.testnet, false), at).toBeNull();
      expect(sendOffer(s, NETWORKS.testnet, true), at).toBeNull();
    }
  });
});

describe("sign and send copy", () => {
  it("drops B7's 'can't sign or send yet' sentence wherever sends are on", () => {
    expect(reservationNote(NETWORKS.mainnet)).toContain("can't sign or send");
    for (const n of [NETWORKS.testnet, NETWORKS.regtest, NETWORKS.devnet]) {
      expect(reservationNote(n), n.kind).not.toContain("can't sign or send");
      expect(reservationNote(NETWORKS.mainnet).startsWith(reservationNote(n)), n.kind).toBe(true);
    }
  });

  it("reads a sent registration's hand-off error as refused or unknown", () => {
    const sent = coordinator({ kind: "sent" }, "wait");
    const refused = (code: string) => (code === "send.no_peers" ? `sentence for ${code}.` : null);
    expect(handoffLine(sent, refused)).toBeNull();
    expect(handoffLine({ ...sent, lastError: "send.no_peers" }, refused)).toBe("The wallet couldn't hand the registration to the network: sentence for send.no_peers.");
    expect(handoffLine({ ...sent, lastError: "send.broadcast_rejected" }, refused)).toBe("The wallet couldn't hand the registration to the network: error send.broadcast_rejected.");
    expect(handoffLine({ ...coordinator({ kind: "combined" }, "send"), lastError: "send.no_peers" }, refused)).toBeNull();
  });

  it("words an unknown hand-off in its own lower-case sentence, never Pay's", () => {
    const sent = coordinator({ kind: "sent" }, "wait");
    const line = (lastError: string) => handoffLine({ ...sent, lastError }, () => "Pay's payment sentence: never used.");
    const cases: [string, string][] = [
      ["send.broadcast_unknown", "The wallet couldn't confirm the registration reached the network: the network didn't answer in time. It may still confirm."],
      ["send.resend_in_progress", "The wallet couldn't confirm the registration reached the network: a resend was still in progress when Dash Wallet last closed. It may still confirm."],
      ["send.some_new_code", "The wallet couldn't confirm the registration reached the network: error send.some_new_code. It may still confirm."],
    ];
    for (const [code, text] of cases) {
      const l = line(code)!;
      expect(l, code).toBe(text);
      expect(l, code).not.toMatch(/payment/i);
      expect(l.split(":").length - 1, code).toBe(1);
    }
  });

  it("shows the way out wherever this wallet holds the registration and Abandon can't stand in", () => {
    const tx = { txid: "7e".repeat(32), discardable: false };
    const row = (x: SharedSession) => renderSession(x, S5_SLICE).row;
    const shown = (x: SharedSession) => wayOutShown(x, row(x));
    expect(shown(coordinator({ kind: "sent" }, "wait", tx)), "sent").toBe("collapsed");
    expect(shown(participantSession("sent", tx)), "sent, participant").toBe("collapsed");
    expect(shown(coordinator({ kind: "sent" }, "wait", { ...tx, txid: null })), "not held").toBeNull();
    const out = participantSession("funding_signed", { ...tx, abandonable: false });
    expect(wayOutShown(out, "signedOut"), "signedOut hidden").toBe("collapsed");
    expect(wayOutShown({ ...out, abandonable: true, abandonCharge: 193 }, "signedOut"), "Abandon offered").toBeNull();
    expect(wayOutShown({ ...out, abandonable: true }, "signedOut"), "abandonable, but the recovery read lists no Abandon").toBe("collapsed");
    const unvalued = { ...out, warnings: [{ kind: "unvalued_inputs" as const, shares: [1], count: 1 }] };
    expect(wayOutShown(unvalued, "sendSigned"), "unvalued_inputs_out hidden").toBe("collapsed");
    expect(wayOutShown({ ...unvalued, txid: null }, "sendSigned"), "not held").toBeNull();
    expect(wayOutShown(out, "sendSigned"), "nothing hidden").toBeNull();
    for (const stage of [{ kind: "confirmed", height: 1 }, { kind: "conflicted" }, { kind: "dead", reason: "inputs_spent" }] as SharedSession["stage"][]) {
      expect(wayOutShown({ ...unvalued, stage }, "confirmed"), stage.kind).toBeNull();
    }
  });

  it("expands the way out once the session is overdue, and only where it is shown (D49)", () => {
    const tx = { txid: "7e".repeat(32), discardable: false };
    const out = participantSession("funding_signed", { ...tx, abandonable: false });
    expect(wayOutShown({ ...out, overdue: true }, "signedOut")).toBe("expanded");
    expect(wayOutShown(coordinator({ kind: "sent" }, "wait", { ...tx, overdue: true }), "sent")).toBe("expanded");
    expect(wayOutShown({ ...out, overdue: true, abandonable: true, abandonCharge: 193 }, "signedOut"), "Abandon offered").toBeNull();
    expect(wayOutShown({ ...out, overdue: true, txid: null }, "signedOut"), "not held").toBeNull();
  });

  it("reads as refused exactly the codes the engine does", () => {
    const rust = (path: string) => readFileSync(new URL(path, import.meta.url), "utf8");
    const list = (src: string, name: string) => [...(new RegExp(`${name}: &\\[&str\\] = &\\[([^\\]]*)\\]`).exec(src)?.[1] ?? "").matchAll(/"([^"]+)"/g)].map((m) => m[1]);
    const inherited = list(rust("../../../../rust/crates/dw-appdb/src/shared_sessions.rs"), "INHERITED_REFUSED_CODES");
    const since = list(rust("../../../../rust/crates/dw-engine/src/send/mod.rs"), "REFUSED_SINCE_INHERITED");
    expect(inherited.length).toBeGreaterThan(0);
    expect(since.length).toBeGreaterThan(0);
    expect([...REFUSED_HANDOFF].sort()).toEqual([...inherited, ...since].sort());
  });

  it("names the sign and send refusals", () => {
    const ctx = { network: "Testnet", dash: (d: number) => `${d}` };
    const err = (code: string, data: unknown = null) => ({ code, message: "", data });
    expect(sharedErrorText(err("app.shared_send_off", { kind: "sharedSendNetwork", network: "mainnet" }), ctx)).toBe("This version sends shared-masternode transactions on test networks only.");
    expect(sharedErrorText(err("masternodes.shared_stage", { kind: "sharedStage", stage: "unsigned" }), ctx)).toContain("Not everyone has signed yet");
    expect(sharedErrorText(err("masternodes.shared_stage", { kind: "sharedStage", stage: "abandoned" }), ctx)).toContain("abandoned");
    expect(sharedErrorText(err("masternodes.shared_message", { kind: "sharedMessage", reason: "conflict", share: null }), ctx)).toContain("conflicts");
    expect(sharedErrorText(err("masternodes.unsupported", { kind: "unsupported", feature: "v24", field: null }), ctx)).not.toContain("{code}");
    expect(sharedErrorText(err("send.grant_exceeded"), ctx)).toBe("The amount changed. Check the figures again.");
  });
});

describe("a session whose funding signature may be out", () => {
  it("never says nothing has left, and never advises Discard", () => {
    const out = { ...participantSession("funding_signed"), next: "copy" as const, discardable: false };
    expect(renderSession(out, NO_SENDS).row).toBe("signedOut");
    expect(renderSession({ ...out, discardable: true }, NO_SENDS).row).toBe("signLater");
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

  it("reads an abandoned session as its spend back, by state, and never says it didn't go out without a refusal", () => {
    const out = participantSession("funding_signed", { txid: "7e".repeat(32), discardable: false });
    const spend = (state: NonNullable<SharedSession["abandonSpend"]>["state"], patch: Partial<SharedSession> = {}): SharedSession => ({
      ...out,
      next: "nothing",
      abandonable: state === "refused" || state === "abandoned_in_activity",
      abandonCharge: state === "refused" || state === "abandoned_in_activity" ? 193 : null,
      resendAbandonCharge: state === "uncertain" || state === "in_history" ? 193 : null,
      abandonSpend: { txid: "a8".repeat(32), state, overdue: false, code: state === "refused" ? "send.broadcast_rejected" : null },
      ...patch,
    });
    const refused = (code: string) => (code === "send.broadcast_rejected" ? "The network refused it." : null);
    expect(renderSession(spend("uncertain"), S5_SLICE)).toMatchObject({ row: "abandoning", copy: false, primary: null, known: true });
    expect(spendBackLine(spend("uncertain"), refused)).toBe("The spend back to this wallet (a8a8a8a8) may already be on the network. The wallet couldn't confirm it went out.");
    expect(spendBackAction(spend("uncertain"))).toBe("resend");
    expect(spendBackLine(spend("refused"), refused)).toBe("The spend back to this wallet didn't go out: The network refused it.");
    expect(spendBackAction(spend("refused"))).toBe("tryAgain");
    expect(spendBackLine(spend("in_history"), refused)).toBe("Abandoning: waiting for the spend back (a8a8a8a8) to confirm. Your coins are still reserved.");
    expect(spendBackAction(spend("in_history"))).toBe("resend");
    expect(spendBackAction(spend("abandoned_in_activity"))).toBe("abandonAgain");
    expect(spendBackAction(spend("legacy"))).toBeNull();
    for (const state of ["uncertain", "in_history", "abandoned_in_activity", "legacy"] as const) {
      expect(spendBackLine(spend(state), refused), state).not.toMatch(/didn't go out/);
    }
    expect(renderSession({ ...spend("in_history"), stage: { kind: "conflicted" } }, S5_SLICE).row, "settled").toBe("conflicted");
  });

  it("reads the Network fee row's value without repeating its label", () => {
    const dash = (d: number) => `${(d / 1e8).toFixed(8)} tDASH`;
    const coord = coordinator({ kind: "draft" }, "copy", { fee: 1_000 });
    expect(feeRowValue(coord, dash, "Alice")).toBe("0.00001000 tDASH, paid by you");
    expect(feeRowValue(participantSession("draft", { fee: 1_000 }), dash, "Alice")).toBe("Paid by Alice");
    expect(feeRowValue({ ...coord, fee: null }, dash, "Alice")).toBeNull();
  });

  it("offers Abandon only while the recovery read lists it", () => {
    const out = participantSession("funding_signed", { txid: "7e".repeat(32), discardable: false });
    expect(out.abandonCharge, "one reserved coin spent whole").toBe(193);
    expect(canAbandon(out)).toBe(true);
    expect(canAbandon({ ...out, abandonCharge: null }), "no Abandon listed").toBe(false);
  });
});

describe("unreadable sessions in progress", () => {
  const row = { id: "a".repeat(64), code: "AAAAAA", kind: "registration" as const, role: "participant" as const, discardable: false, abandonable: false, abandonCharge: null, reserved: ["x-0"], updatedAt: 1 };
  it("lists one it may discard or one still holding coins, and drops one whose locks were released", () => {
    expect(unreadableInProgress([row])).toHaveLength(1);
    expect(unreadableInProgress([{ ...row, discardable: true, reserved: [] }])).toHaveLength(1);
    expect(unreadableInProgress([{ ...row, reserved: [] }])).toEqual([]);
    expect(unreadableInProgress(undefined)).toEqual([]);
  });
});
