// The lock report's consumer (E0-13 D3). A `LockOutcome` is bounded: at most 16 flows of 8
// artifacts and 16 wallets, the rest only counted (`flowsOmitted`, `artifactsOmitted`,
// `walletsWithSecretsOmitted`). A report with any of those above 0 is not complete, so what it
// leaves out is unresolved: the renderer never reads "nothing pending" from it, and reconciles
// through `leases`, `dispatchStatus` and `lockStatus` instead.

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

export interface LockReportSummary {
  /** Nothing was left out: every flow, artifact and wallet is listed. */
  complete: boolean;
  omitted: { flows: number; artifacts: number; wallets: number };
  /** Listed flows something of which may still reach the network. */
  pending: FlowLockReport[];
  /** Something may still be sent, or the report cannot say: reconcile before saying otherwise. */
  unresolved: boolean;
}

export function summarizeLockReport(report: LockOutcome): LockReportSummary {
  const omitted = {
    flows: report.flowsOmitted,
    artifacts: report.flows.reduce((n, f) => n + f.artifactsOmitted, 0),
    wallets: report.walletsWithSecretsOmitted,
  };
  const complete = omitted.flows === 0 && omitted.artifacts === 0 && omitted.wallets === 0;
  // A flow's `outcome` covers all its artifacts, listed or not, so it decides "pending".
  const pending = report.flows.filter((f) => PENDING.has(f.outcome));
  return { complete, omitted, pending, unresolved: !complete || pending.length > 0 };
}

/** True when a report has anything to show: a flow it cut, or something it left out. */
export function reportsAnything(report: LockOutcome): boolean {
  return report.flows.length > 0 || !summarizeLockReport(report).complete;
}

export interface Reconciled {
  /** The listed pending artifacts, with what the engine knows of each now. */
  artifacts: { walletId: string; artifact: string; state: DispatchStateName | null }[];
  /** Leases that may still hand something off. */
  live: Lease[];
  /** Every wallet with secrets (the report lists at most 16). */
  lock: LockStatus;
  /** Every pending artifact settled (`sent` or `notSent`), and, when the report left
   * something out, no lease that could still hand anything off. */
  settled: boolean;
}

/** A lease that could still hand something off: a call or permit in flight, or committed funds
 * on a flow that has not ended. */
function mayStillSend(l: Lease): boolean {
  return l.inFlight > 0 || l.callRunning || (l.fundsCommitted && l.state.kind !== "ended");
}

export async function reconcileLockReport(
  report: LockOutcome,
  backend: Pick<Backend, "leases" | "dispatchStatus" | "lockStatus">,
): Promise<Reconciled> {
  const summary = summarizeLockReport(report);
  const wanted = summary.pending.flatMap((f) => f.artifacts.filter((a) => PENDING.has(a.outcome)).map((a) => ({ walletId: f.walletId, artifact: a.artifact })));
  const [leases, lock, states] = await Promise.all([
    backend.leases(),
    backend.lockStatus(),
    Promise.all(wanted.map((a) => backend.dispatchStatus(a.walletId, a.artifact))),
  ]);
  const artifacts = wanted.map((a, i) => ({ ...a, state: states[i] }));
  const live = leases.filter(mayStillSend);
  const artifactsSettled = artifacts.every((a) => a.state === "sent" || a.state === "notSent");
  // A pending flow listed without its pending artifacts (cut by `artifactsOmitted`) has
  // nothing to ask `dispatchStatus` about: only its lease can say.
  const pendingWithoutIds = summary.pending.some((f) => !f.artifacts.some((a) => PENDING.has(a.outcome)));
  const settled = artifactsSettled && ((summary.complete && !pendingWithoutIds) || live.length === 0);
  return { artifacts, live, lock, settled };
}
