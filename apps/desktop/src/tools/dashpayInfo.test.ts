// Tools ▸ Information's DashPay card (DP6-03, F23): each state, its precedence, and each row's
// format. Test names lead with the design note's D-row or the parity row they cover.

import { describe, expect, it } from "vitest";
import type { DashPayStatus } from "../api/generated/DashPayStatus";
import type { DashPaySyncStatus } from "../api/generated/DashPaySyncStatus";
import type { StartupStatus } from "../api/generated/StartupStatus";
import type { SyncLoopStatus } from "../api/generated/SyncLoopStatus";
import { COPY } from "../dashpay/status";
import { clockText, dashpayInfo, INFO_COPY, lastPassRow, loopRow, quorumRow, STARTUP_TEXT, type DashPayInfoInputs } from "./dashpayInfo";
import { NONE } from "./information";

const TZ = "UTC";
/** 2026-10-06 12:00:00 UTC. */
const NOW = Date.UTC(2026, 9, 6, 12) / 1000;

const loop = (syncLoop: SyncLoopStatus["syncLoop"], running = true, lastRunAt: number | null = NOW - 5, nextRunAt: number | null = NOW + 10): SyncLoopStatus => ({
  syncLoop,
  running,
  lastRunAt,
  nextRunAt: running ? nextRunAt : null,
});

const LOOPS = [loop("identitySync"), loop("dashPaySync"), loop("dpnsSync")];

const sync = (over: Partial<DashPaySyncStatus> = {}): DashPaySyncStatus => ({
  startup: "ready",
  lastPass: null,
  pendingContactCrypto: 0,
  loops: LOOPS,
  quorumSource: "spv",
  ...over,
});

const READY: DashPayStatus = { kind: "ready", main: "AliceId" };

const inputs = (over: Partial<DashPayInfoInputs> = {}): DashPayInfoInputs => ({
  noWallet: false,
  status: READY,
  sync: sync(),
  syncError: null,
  quorumSource: undefined,
  trustedOnly: false,
  now: NOW,
  timeZone: TZ,
  ...over,
});

const stateOf = (over: Partial<DashPayInfoInputs>) => dashpayInfo(inputs(over)).state;

describe("the card's state (D2)", () => {
  it("F23: SPV quorum keys and an identity are Running, with Sync now", () => {
    const c = dashpayInfo(inputs());
    expect(c).toMatchObject({ state: "running", tone: "success", chip: "Running", headline: INFO_COPY.running, syncNow: { disabled: false, note: null } });
    expect(c.rows.map((r) => r.title)).toEqual([
      "Identity scan",
      "Last pass",
      "Contacts waiting for an unlock",
      "Identities loop",
      "Contacts and payments loop",
      "Username contests loop",
      "Quorum keys",
    ]);
  });

  it("D5: quorum keys not from SPV are degraded, never Running", () => {
    const degraded = dashpayInfo(inputs({ sync: sync({ quorumSource: "trusted" }) }));
    expect(degraded).toMatchObject({ state: "degraded", tone: "warning", chip: "Degraded", headline: INFO_COPY.degraded.trusted });
    expect(degraded.line).toBe(quorumRow("trusted", false).note);
    expect(dashpayInfo(inputs({ sync: sync({ quorumSource: "trustedFallback" }) }))).toMatchObject({ state: "degraded", headline: INFO_COPY.degraded.trustedFallback });
    expect(dashpayInfo(inputs({ sync: sync({ quorumSource: "trusted" }), trustedOnly: true }))).toMatchObject({ state: "degraded", headline: INFO_COPY.degraded.trustedOnly });
    // The toggle unread reads as the degraded mode, the default (DEC-168).
    expect(dashpayInfo(inputs({ sync: sync({ quorumSource: "trusted" }), trustedOnly: undefined })).headline).toBe(INFO_COPY.degraded.trusted);
    // Every kind of registered identity.
    for (const status of [READY, { kind: "registering", draft: "d1" }, { kind: "contestPending", identity: "A", label: "alice", endsAt: null }] as DashPayStatus[])
      expect(stateOf({ status, sync: sync({ quorumSource: "trusted" }) })).toBe("degraded");
  });

  it("D2: no identity is Not set up, whatever the quorum source", () => {
    const c = dashpayInfo(inputs({ status: { kind: "noIdentity", reason: null }, sync: sync({ startup: "noIdentity", quorumSource: "trusted" }) }));
    expect(c).toMatchObject({ state: "notSetUp", tone: "neutral", headline: INFO_COPY.notSetUp.title, line: INFO_COPY.notSetUp.message, syncNow: { disabled: false } });
    expect(dashpayInfo(inputs({ status: { kind: "noIdentity", reason: "waitingForSync" }, sync: sync({ startup: "noIdentity" }) })).line).toBe(INFO_COPY.notSetUpWaiting);
  });

  it("D2: a watch-only wallet is Watch-only, its Sync now held with the reason", () => {
    const c = dashpayInfo(inputs({ status: { kind: "noIdentity", reason: "watchOnly" }, sync: sync({ startup: "noIdentity", quorumSource: "trusted" }) }));
    expect(c).toMatchObject({ state: "watchOnly", tone: "neutral", headline: COPY.watchOnly.title, syncNow: { disabled: true, note: INFO_COPY.watchOnly } });
  });

  it("D2: a failed bring-up is Failed, before anything but watch-only", () => {
    const failed: StartupStatus[] = ["discoveryFailed", "identityScanIncomplete", "partialNoIdentity", "partialAccountsPending"];
    for (const startup of failed) {
      const c = dashpayInfo(inputs({ sync: sync({ startup }), status: { kind: "startupIncomplete", startup } }));
      expect(c, startup).toMatchObject({ state: "failed", tone: "danger", headline: COPY.lookFailed.title, line: COPY.lookFailed.message });
    }
    expect(dashpayInfo(inputs({ sync: sync({ startup: "seedBindingUnverified" }) }))).toMatchObject({ state: "failed", headline: COPY.unverified.title });
    // A failure outranks a healthy identity and SPV quorum keys.
    expect(stateOf({ status: READY, sync: sync({ startup: "discoveryFailed" }) })).toBe("failed");
  });

  it("D4: a failed last pass is Failed, with its code", () => {
    const lastPass = { startedAt: NOW - 8, finishedAt: NOW - 5, newRequests: 0, newContacts: 0, newPayments: 0, failureCode: "platform.unavailable" };
    const c = dashpayInfo(inputs({ sync: sync({ lastPass }) }));
    expect(c).toMatchObject({ state: "failed", headline: INFO_COPY.passFailed.title, line: "It stopped with platform.unavailable.", syncNow: { disabled: false } });
  });

  it("D2: no bring-up is Not started, a running one or an unsettled status Starting", () => {
    expect(dashpayInfo(inputs({ sync: sync({ startup: "notRun" }), status: { kind: "startupIncomplete", startup: "notRun" } }))).toMatchObject({
      state: "notStarted",
      headline: COPY.notStarted.title,
    });
    expect(dashpayInfo(inputs({ sync: sync({ startup: "starting" }), status: { kind: "startupIncomplete", startup: "starting" } }))).toMatchObject({
      state: "starting",
      headline: COPY.looking.title,
      busy: true,
    });
    expect(dashpayInfo(inputs({ sync: sync({ startup: "identityUnsettled" }) }))).toMatchObject({ state: "starting", headline: COPY.unlockToLook.title, busy: false });
    // BL-153 D16: an outcome read from a snapshot, no identity known yet.
    expect(stateOf({ sync: sync({ startup: "noIdentity" }), status: { kind: "startupIncomplete", startup: "noIdentity" } })).toBe("starting");
  });

  it("D2: the status unread leaves the decision to the sync status", () => {
    expect(stateOf({ status: undefined })).toBe("running");
    expect(stateOf({ status: undefined, sync: sync({ quorumSource: "trusted" }) })).toBe("degraded");
  });

  it("D2: no wallet shows the session's quorum source alone, with no Sync now", () => {
    const c = dashpayInfo(inputs({ noWallet: true, sync: undefined, status: undefined, quorumSource: "trusted" }));
    expect(c).toMatchObject({ state: "noWallet", headline: INFO_COPY.noWallet, syncNow: null });
    expect(c.rows).toEqual([quorumRow("trusted", false)]);
  });

  it("D7: unread is Reading, a failed read Unread with the reason and no rows", () => {
    expect(dashpayInfo(inputs({ sync: undefined }))).toMatchObject({ state: "loading", busy: true, rows: [], syncNow: null });
    expect(dashpayInfo(inputs({ sync: undefined, syncError: "boom" }))).toMatchObject({ state: "error", tone: "danger", headline: INFO_COPY.error, line: "boom", rows: [], syncNow: null });
  });

  it("D3: Sync now is held while the DashPay loop does not run", () => {
    const stopped = LOOPS.map((l) => ({ ...l, running: false, nextRunAt: null }));
    expect(dashpayInfo(inputs({ sync: sync({ loops: stopped }) })).syncNow).toEqual({ disabled: true, note: INFO_COPY.loopsStopped });
    // The other loops running do not make a pass possible.
    const onlyDashPayStopped = LOOPS.map((l) => (l.syncLoop === "dashPaySync" ? { ...l, running: false } : l));
    expect(dashpayInfo(inputs({ sync: sync({ loops: onlyDashPayStopped }) })).syncNow?.disabled).toBe(true);
    expect(dashpayInfo(inputs({ sync: sync({ loops: [] }) })).syncNow?.disabled).toBe(true);
  });
});

describe("the rows (D4, D5)", () => {
  it("F23: the scan verdict names every bring-up outcome", () => {
    for (const startup of Object.keys(STARTUP_TEXT) as StartupStatus[]) {
      const row = dashpayInfo(inputs({ sync: sync({ startup }) })).rows[0];
      expect(row).toEqual({ title: "Identity scan", ...STARTUP_TEXT[startup] });
    }
    expect(STARTUP_TEXT.ready.value).toBe("Complete: identity found");
  });

  it("D4: the last pass is a dash until one reports, then its end and its findings", () => {
    expect(lastPassRow(null, TZ)).toEqual({ title: "Last pass", value: NONE, note: "No pass has reported yet." });
    const pass = { startedAt: NOW - 8, finishedAt: NOW - 5, newRequests: 2, newContacts: 1, newPayments: 0, failureCode: null };
    expect(lastPassRow(pass, TZ)).toEqual({ title: "Last pass", value: "Tue Oct 6 11:59:55 2026", note: "2 new requests, 1 new contact" });
    expect(lastPassRow({ ...pass, newRequests: 0, newContacts: 0, newPayments: 1 }, TZ).note).toBe("1 new payment");
    expect(lastPassRow({ ...pass, newRequests: 0, newContacts: 0 }, TZ).note).toBe("Nothing new");
    expect(lastPassRow({ ...pass, failureCode: "platform.timeout" }, TZ).note).toBe("Failed: platform.timeout");
  });

  it("F23: contacts waiting for an unlock, None at zero", () => {
    const pending = (n: number) => dashpayInfo(inputs({ sync: sync({ pendingContactCrypto: n }) })).rows[2];
    expect(pending(0)).toEqual({ title: "Contacts waiting for an unlock", value: "None" });
    expect(pending(3).value).toBe("3");
  });

  it("F23: a loop says whether it runs, its last run and its next", () => {
    expect(loopRow(loop("dashPaySync"), NOW, TZ)).toEqual({ title: "Contacts and payments loop", value: "Running · last run 11:59:55 · next 12:00:10" });
    expect(loopRow(loop("identitySync", true, null, null), NOW, TZ).value).toBe("Running · no run yet");
    expect(loopRow(loop("dpnsSync", true, NOW - 5, null), NOW, TZ).value).toBe("Running · last run 11:59:55");
    expect(loopRow(loop("dpnsSync", false, NOW - 120), NOW, TZ)).toEqual({ title: "Username contests loop", value: "Stopped · last run 11:58:00" });
    expect(loopRow(loop("platformAddressSync", false, null), NOW, TZ)).toEqual({ title: "Platform addresses loop", value: "Stopped" });
  });

  it("F23: a run on another day shows its date", () => {
    expect(clockText(NOW - 86_400, NOW, TZ)).toBe("Mon Oct 5 12:00:00 2026");
    expect(clockText(NOW - 60, NOW, TZ)).toBe("11:59:00");
  });

  it("D5: the quorum source and the toggle", () => {
    expect(quorumRow("spv", false)).toEqual({ title: "Quorum keys", value: "SPV (verified masternode list)" });
    expect(quorumRow("trustedFallback", false).value).toBe("Trusted service until masternode sync finishes");
    expect(quorumRow("trusted", false).value).toBe("Trusted service (degraded mode)");
    expect(quorumRow("trusted", true).value).toBe("Trusted service only (developer setting)");
    // The toggle unread is the default degraded mode, not the developer setting.
    expect(quorumRow("trusted", undefined).value).toBe("Trusted service (degraded mode)");
    expect(quorumRow(undefined, false)).toEqual({ title: "Quorum keys", value: NONE });
    // The card's row is the sync status's source, not the session read.
    expect(dashpayInfo(inputs({ quorumSource: "trusted" })).rows.at(-1)!.value).toBe("SPV (verified masternode list)");
  });
});
