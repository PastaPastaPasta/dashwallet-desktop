import { describe, expect, it } from "vitest";
import type { IdentitySummary } from "../api/generated/IdentitySummary";
import type { RegistrationStatus } from "../api/generated/RegistrationStatus";
import type { DashPayStatus } from "../api/generated/DashPayStatus";
import { homeCard, type StatusInputs } from "../dashpay/status";
import { chipState, sidebarJoins } from "./shell";

const alice = (mainName: string | null, namesUpdating = false, namesCheck: IdentitySummary["namesCheck"] = null) =>
  ({ identity: "A", mainName, namesUpdating, namesCheck }) as IdentitySummary;

describe("identity chip (DASHPAY §4, U-01 D1)", () => {
  it("F1 follows the DashPay status: Registering, @name · Pending, the main identity", () => {
    // No identity: Home's card and the sidebar ask to join, not the toolbar too.
    expect(chipState({ kind: "noIdentity", reason: null }, undefined)).toBeNull();
    expect(chipState({ kind: "noIdentity", reason: "waitingForSync" }, undefined)).toBeNull();
    expect(chipState({ kind: "registering", draft: "1" }, undefined)).toEqual({ kind: "registering", label: null });
    expect(chipState({ kind: "contestPending", identity: "A", label: "alice", endsAt: null }, alice("alice007"))).toEqual({ kind: "contest", label: "alice" });
    expect(chipState({ kind: "ready", main: "A" }, alice("alice"))).toEqual({ kind: "identity", identity: "A", username: "alice", names: "named" });
    // DashPay starting, or a status that cannot be read: no chip.
    expect(chipState({ kind: "startupIncomplete", startup: "notRun" }, undefined)).toBeNull();
    expect(chipState(undefined, undefined)).toBeNull();
  });

  it("BL-153 D12, DP1-08 D23: a nameless identity says so only once its names were fetched", () => {
    const ready = { kind: "ready", main: "A" } as const;
    expect(chipState(ready, alice(null))).toEqual({ kind: "identity", identity: "A", username: null, names: "none" });
    expect(chipState(ready, alice(null, true))).toEqual({ kind: "identity", identity: "A", username: null, names: "updating" });
    expect(chipState(ready, undefined)).toEqual({ kind: "identity", identity: "A", username: null, names: "updating" });
    expect(chipState(ready, alice(null, false, "pending"))).toEqual({ kind: "identity", identity: "A", username: null, names: "updating" });
    expect(chipState(ready, alice(null, false, "failed"))).toEqual({ kind: "identity", identity: "A", username: null, names: "unknown" });
  });

  it("DP1-08: a watch-only wallet has no chip; a registration names its username once its row is read", () => {
    expect(chipState({ kind: "noIdentity", reason: "watchOnly" }, undefined)).toBeNull();
    const row = { draft: "1", label: "bob" } as RegistrationStatus;
    expect(chipState({ kind: "registering", draft: "1" }, undefined, row)).toEqual({ kind: "registering", label: "bob" });
    // Another draft's row says nothing about this one.
    expect(chipState({ kind: "registering", draft: "2" }, undefined, row)).toEqual({ kind: "registering", label: null });
  });
});

describe("Join DashPay asks once (Home: the card and the sidebar, not the toolbar)", () => {
  const inputs = (status: DashPayStatus, main?: IdentitySummary): StatusInputs => ({
    status,
    statusError: false,
    main,
    registration: undefined,
    lock: "unlocked",
    syncPercent: null,
    date: String,
    amount: String,
    discreet: false,
  });
  /** Where Home and the shell around it lead into Join DashPay; the toolbar chip never does. */
  const joins = (status: DashPayStatus, main?: IdentitySummary) =>
    [homeCard(inputs(status, main))?.action?.kind === "join" && "card", sidebarJoins(status) && "sidebar"].filter(Boolean);

  it("no identity: one button on Home's card, plus the sidebar entry", () => {
    expect(joins({ kind: "noIdentity", reason: null })).toEqual(["card", "sidebar"]);
    expect(chipState({ kind: "noIdentity", reason: null }, undefined)).toBeNull();
    const card = homeCard(inputs({ kind: "noIdentity", reason: null }))!;
    expect(card.title).not.toBe("Join DashPay");
    expect(card.action?.label).toBe("Join DashPay");
  });

  it("registering: the sidebar still leads to the Join page; the card and the chip show progress", () => {
    const registering: DashPayStatus = { kind: "registering", draft: "1" };
    expect(joins(registering)).toEqual(["sidebar"]);
    expect(homeCard(inputs(registering))).toMatchObject({ state: "registering", action: null });
    expect(chipState(registering, undefined)).toEqual({ kind: "registering", label: null });
  });

  it("registered: no Join DashPay anywhere; Contacts and the profile chip", () => {
    const ready: DashPayStatus = { kind: "ready", main: "A" };
    expect(joins(ready, alice("alice"))).toEqual([]);
    expect(homeCard(inputs(ready, alice("alice")))).toBeNull();
    expect(chipState(ready, alice("alice"))).toMatchObject({ kind: "identity", username: "alice" });
  });
});
