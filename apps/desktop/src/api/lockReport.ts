// The lock report's consumer (E0-13 D3). A `LockOutcome` is bounded: at most 16 flows of 8
// artifacts and 16 wallets, the rest only counted (`flowsOmitted`, `artifactsOmitted`,
// `walletsWithSecretsOmitted`). A report with any of those above 0 is not complete, so what it
// leaves out is unresolved: the renderer never reads "nothing pending" from it, and reconciles
// through `leases`, `dispatchStatus` and `lockStatus` instead. Omitted flows and artifacts have
// no ids, so they settle only when the report's order proves none of them is pending.
//
// A flow's `outcome` is the strongest of its artifacts' (`sent` > `willBeSent` > `maybeSent` >
// `cancelled`, E0-04 §8.4), so it does not say whether anything of the flow is still pending: a
// `sent` registration may hold an identity call still running (`maybeSent`). Only `cancelled`
// proves a flow has nothing pending; each artifact's own outcome says the rest.

import type { Backend } from "./backend";
import type { DispatchStateName } from "./generated/DispatchStateName";
import type { FlowKindName } from "./generated/FlowKindName";
import type { FlowLockReport } from "./generated/FlowLockReport";
import type { FlowOutcomeName } from "./generated/FlowOutcomeName";
import type { Lease } from "./generated/Lease";
import type { LockOutcome } from "./generated/LockOutcome";
import type { LockStatus } from "./generated/LockStatus";

/** E0-04 §16.10 C4 after a manual lock, by outcome. */
export const OUTCOME_LINE: Record<FlowOutcomeName, string> = {
  cancelled: "Cancelled",
  maybeSent: "May have been sent",
  willBeSent: "Will be sent",
  sent: "Sent",
};

export const FLOW_NAME: Record<FlowKindName, string> = {
  registration: "Username registration",
  topUp: "Credit top-up",
  withdraw: "Credit withdrawal",
  nameRegistration: "Name registration",
  profileEdit: "Profile update",
  contactRequest: "Contact request",
  accept: "Accepting a request",
  acceptAndPay: "Accept and pay",
  privateDetails: "Private contact details",
  enableDashPayKeys: "Enabling DashPay keys",
};

const PENDING: ReadonlySet<FlowOutcomeName> = new Set(["maybeSent", "willBeSent"]);
const isPending = (x: { outcome: FlowOutcomeName }) => PENDING.has(x.outcome);

/** Whether a flow's artifacts, cut to the listed ones, may have left a pending one out. They are
 * listed unresolved first by their own outcome (D3), so a settled last one settles the rest. */
const artifactTailMayBePending = (f: FlowLockReport) =>
  f.artifactsOmitted > 0 && (f.artifacts.length === 0 || isPending(f.artifacts[f.artifacts.length - 1]));

/** Whether flows cut from the list may have something pending. Flows are listed by their outcome,
 * the strongest of their artifacts', so only a `cancelled` last one proves the rest cancelled. */
const flowTailMayBePending = (report: LockOutcome) =>
  report.flowsOmitted > 0 && (report.flows.length === 0 || report.flows[report.flows.length - 1].outcome !== "cancelled");

/** Something of the flow may still reach the network: a listed pending artifact, an omitted one
 * that may be, or a pending outcome (which the strongest of its artifacts' then is). */
const mayBePending = (f: FlowLockReport) => isPending(f) || f.artifacts.some(isPending) || artifactTailMayBePending(f);

export interface LockReportSummary {
  /** Nothing was left out: every flow, artifact and wallet is listed. */
  complete: boolean;
  omitted: { flows: number; artifacts: number; wallets: number };
  /** Listed flows something of which may still reach the network. */
  pending: FlowLockReport[];
  /** Something may still be sent, or the report cannot say: reconcile before saying otherwise. */
  unresolved: boolean;
  /** Something pending that no query can name: omitted flows after a flow not `cancelled`, an
   * omitted artifact after a pending one, or a pending flow with no pending artifact listed.
   * Nothing the renderer can read settles it. */
  beyondReach: boolean;
}

export function summarizeLockReport(report: LockOutcome): LockReportSummary {
  const omitted = {
    flows: report.flowsOmitted,
    artifacts: report.flows.reduce((n, f) => n + f.artifactsOmitted, 0),
    wallets: report.walletsWithSecretsOmitted,
  };
  const complete = omitted.flows === 0 && omitted.artifacts === 0 && omitted.wallets === 0;
  const pending = report.flows.filter(mayBePending);
  const beyondReach =
    flowTailMayBePending(report) || report.flows.some((f) => artifactTailMayBePending(f) || (isPending(f) && !f.artifacts.some(isPending)));
  return { complete, omitted, pending, unresolved: !complete || pending.length > 0, beyondReach };
}

/** True when a report has anything to show: a flow it cut, or something it left out. */
export function reportsAnything(report: LockOutcome): boolean {
  return report.flows.length > 0 || !summarizeLockReport(report).complete;
}

export interface Reconciled {
  /** The listed pending artifacts, with what the engine knows of each now (`null`: unknown,
   * including a failed query). */
  artifacts: { walletId: string; artifact: string; state: DispatchStateName | null }[];
  /** Leases with a call or permit in flight: something is being handed off now. */
  live: Lease[];
  /** Every wallet with secrets: the report lists at most 16 (`walletsWithSecretsOmitted`). */
  lock: LockStatus;
  /** Nothing the report covers can still be sent: no pending item beyond reach, every listed
   * pending artifact `sent` or `notSent`, and no hand-off in flight. A lease's absence or end
   * never settles anything by itself: a revoked flow without committed funds, or one the reaper
   * ended, may still have a state transition or transaction out. */
  settled: boolean;
}

export async function reconcileLockReport(
  report: LockOutcome,
  backend: Pick<Backend, "leases" | "dispatchStatus" | "lockStatus">,
): Promise<Reconciled> {
  const summary = summarizeLockReport(report);
  const wanted = report.flows.flatMap((f) => f.artifacts.filter(isPending).map((a) => ({ walletId: f.walletId, artifact: a.artifact })));
  const [leases, lock, states] = await Promise.all([
    backend.leases(),
    backend.lockStatus(),
    // One failed lookup (a wallet removed since the lock) leaves that artifact unknown.
    Promise.all(wanted.map((a) => backend.dispatchStatus(a.walletId, a.artifact).catch(() => null))),
  ]);
  const artifacts = wanted.map((a, i) => ({ ...a, state: states[i] }));
  const live = leases.filter((l) => l.inFlight > 0 || l.callRunning);
  const settled = !summary.beyondReach && live.length === 0 && artifacts.every((a) => a.state === "sent" || a.state === "notSent");
  return { artifacts, live, lock, settled };
}
