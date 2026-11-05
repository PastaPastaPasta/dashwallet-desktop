// Unfinished transfers on Home (engine `unfinished_transfers` / `finish_transfer`): Core → Shielded
// locks a wallet restore read back from the chain, which Platform is not known to have applied.
// The Home card's view (which copy and actions, if any), the sheet's rows, and the model that
// finishes them: one lock under a one-duff grant, or all of them with Repair's "Finish transfers".
// Only a proved apply finishes a lock; "checking" means it may already be in the balance.

import type { Backend } from "../api/backend";
import type { AppError } from "../api/generated/AppError";
import type { LockFinish } from "../api/generated/LockFinish";
import type { LockKind } from "../api/generated/LockKind";
import type { UnfinishedLock } from "../api/generated/UnfinishedLock";
import type { UnfinishedState } from "../api/generated/UnfinishedState";
import type { UnfinishedTransfers } from "../api/generated/UnfinishedTransfers";
import { REG_COPY } from "../dashpay/registration";
import { platformOp } from "../dashpay/topup";
import { finishGrant } from "../dashpay/wizard";
import { Model } from "../onboarding/model";
import { GrantModel, PAYMENT_PROMPT } from "../tools/grant";

export type RecoveryApi = Pick<
  Backend,
  "finishTransfer" | "finishAssetLocks" | "registrationQuote" | "authorize" | "revokeGrant" | "lockStatus" | "paymentAuthentication" | "unlock" | "unlockMixingOnly"
>;

export const RECOVERY_COPY = {
  title: "Unfinished transfers",
  one: (amount: string) => `${amount} was locked for your Shielded balance and isn't added yet.`,
  several: (n: number, total: string) => `${n} transfers from before the restore aren't finished (${total}).`,
  running: "Finishing…",
  checking: (n: number) => `${n} ${n === 1 ? "transfer may" : "transfers may"} already be finished. We can't confirm it yet.`,
  finish: "Finish",
  finishAll: "Finish all",
  checkAgain: "Check again",
  details: "Details",
  retry: "Retry",
  dismiss: "Dismiss",
  sheet: {
    title: "Unfinished transfers",
    intro: "These transfers were locked before the restore. Finishing adds them to Shielded.",
    footer: "Nothing more is paid: finishing only adds money that's already locked.",
    close: "Close",
    kind: { shielded: "Shielded", platformAddress: "Platform address" } satisfies Record<LockKind, string>,
    state: {
      ready: "Ready",
      checking: "Checking",
      reportedUsed: "Checking",
      confirming: "Waiting for the network to confirm",
      notActive: "Not active on this network yet",
    } satisfies Record<UnfinishedState, string>,
    done: {
      applied: "Added to Shielded",
      checking: "Checking: this amount may already be in your balance; we can't confirm it yet.",
      proofWaiting: "Waiting for the network to confirm",
    },
  },
} as const;

export type CardAction = "finish" | "finishAll" | "checkAgain" | "details" | "retry" | "dismiss";

export interface CardView {
  kind: "running" | "failed" | "ready" | "checking" | "done";
  text: string;
  /** The result line of the last Finish all, shown above the rest. */
  note: string | null;
  actions: CardAction[];
  /** The lock "Finish" is for. */
  lock: UnfinishedLock | null;
}

type Action = { kind: "finish"; lock: UnfinishedLock } | { kind: "all" } | { kind: "check"; locks: UnfinishedLock[] };

export type RecoveryState =
  | { kind: "idle" }
  | { kind: "running" }
  | { kind: "failed"; error: unknown; retry: Action }
  | { kind: "done"; text: string };

/** What happened to one lock in this session. */
export type RowResult = LockFinish;

export const lockKey = (l: Pick<UnfinishedLock, "kind" | "txid" | "vout">) => `${l.kind}:${l.txid}:${l.vout}`;

export const restoredOf = (data: UnfinishedTransfers | null | undefined): UnfinishedLock[] => data?.locks.filter((l) => l.origin === "restored") ?? [];

/** The Home card, `null` for none. It shows when the list is read and complete and holds a restored
 * lock that is ready or checking, and while a finish from the card runs, failed or reported. */
export function cardView(
  data: UnfinishedTransfers | null | undefined,
  state: RecoveryState,
  duffs: (d: number) => string,
  errorText: (e: unknown) => string,
): CardView | null {
  if (state.kind === "running") return { kind: "running", text: RECOVERY_COPY.running, note: null, actions: [], lock: null };
  if (state.kind === "failed") return { kind: "failed", text: errorText(state.error), note: null, actions: ["retry"], lock: null };
  const note = state.kind === "done" ? state.text : null;
  const restored = data?.complete ? restoredOf(data) : [];
  const ready = restored.filter((l) => l.state === "ready");
  const checking = restored.filter((l) => l.state === "checking");
  const details: CardAction[] = restored.length > 1 ? ["details"] : [];
  if (ready.length === 1) return { kind: "ready", text: RECOVERY_COPY.one(duffs(ready[0].duffs)), note, actions: ["finish", ...details], lock: ready[0] };
  if (ready.length > 1) {
    const total = ready.reduce((n, l) => n + l.duffs, 0);
    return { kind: "ready", text: RECOVERY_COPY.several(ready.length, duffs(total)), note, actions: ["finishAll", "details"], lock: null };
  }
  if (checking.length > 0) return { kind: "checking", text: RECOVERY_COPY.checking(checking.length), note, actions: ["checkAgain", "details"], lock: null };
  return note === null ? null : { kind: "done", text: note, note: null, actions: ["dismiss"], lock: null };
}

/** The sheet's rows: the restored locks, and the ones this session applied (they have left the list). */
export function sheetRows(data: UnfinishedTransfers | null | undefined, results: ReadonlyMap<string, { lock: UnfinishedLock; result: RowResult }>) {
  const rows = restoredOf(data).map((lock) => ({ lock, result: results.get(lockKey(lock))?.result ?? null }));
  const gone = [...results.values()].filter((r) => r.result.kind === "applied" && !rows.some((x) => lockKey(x.lock) === lockKey(r.lock)));
  return [...rows, ...gone.map((r) => ({ lock: r.lock, result: r.result as RowResult | null }))];
}

/** A row's done line. */
export function resultText(r: RowResult, errorText: (e: unknown) => string): string {
  switch (r.kind) {
    case "applied":
      return RECOVERY_COPY.sheet.done.applied;
    case "checking":
    case "reportedUsed":
      return RECOVERY_COPY.sheet.done.checking;
    case "proofWaiting":
      return RECOVERY_COPY.sheet.done.proofWaiting;
    case "notApplied":
      return errorText(engineError(r.error));
  }
}

/** A `notApplied` code as the error the app's error text reads. */
export function engineError(code: string): AppError {
  return { code, message: code, data: null } as unknown as AppError;
}

const settledListeners = new Set<() => void>();

/** Called whenever a finish that ran has ended (api/events.ts re-reads `["dashpay"]` from it, so the
 * list and the Shielded balance follow). Returns the unsubscribe. */
export function onRecoverySettled(f: () => void): () => void {
  settledListeners.add(f);
  return () => settledListeners.delete(f);
}

export class RecoveryModel extends Model {
  state: RecoveryState = { kind: "idle" };
  /** Per lock: what its last finish came to in this session. */
  results = new Map<string, { lock: UnfinishedLock; result: RowResult }>();
  readonly grants: GrantModel;

  constructor(
    private readonly api: RecoveryApi,
    readonly walletId: string,
  ) {
    super();
    this.grants = new GrantModel(api);
    this.grants.prompt = PAYMENT_PROMPT;
    this.grants.subscribe(() => this.changed());
  }

  get running(): boolean {
    return this.state.kind === "running";
  }

  /** Finishes one ready lock. */
  finish(lock: UnfinishedLock): Promise<void> {
    return this.#run({ kind: "finish", lock });
  }

  /** Repair's "Finish transfers": every unfinished transfer, registrations' and locks'. */
  finishAll(): Promise<void> {
    return this.#run({ kind: "all" });
  }

  /** Reads each lock again, one after the other. */
  checkAgain(locks: UnfinishedLock[]): Promise<void> {
    return this.#run({ kind: "check", locks });
  }

  retry(): Promise<void> {
    return this.state.kind === "failed" ? this.#run(this.state.retry) : Promise.resolve();
  }

  dismiss() {
    if (this.state.kind === "done" || this.state.kind === "failed") this.#set({ kind: "idle" });
  }

  async #run(action: Action) {
    if (this.running) return;
    const since = this.grants.epoch;
    this.#set({ kind: "running" });
    let ran = false;
    try {
      const next = await this.#do(action, since, () => (ran = true));
      this.#set(next);
    } catch (e) {
      this.#set({ kind: "failed", error: e, retry: action });
    } finally {
      if (ran) for (const f of settledListeners) queueMicrotask(f);
    }
  }

  async #do(action: Action, since: number, started: () => void): Promise<RecoveryState> {
    if (action.kind === "all") {
      const caps = await finishGrant(this.api, this.walletId);
      const report = await this.grants.withGrant(
        { kind: "platformOp", maxDuffs: caps.maxDuffs, maxCredits: caps.maxCredits },
        this.walletId,
        (grant) => (started(), this.api.finishAssetLocks(this.walletId, grant.id)),
        since,
      );
      return report ? { kind: "done", text: REG_COPY.finished(report.value) } : { kind: "idle" };
    }
    for (const lock of action.kind === "finish" ? [action.lock] : action.locks) {
      const got = await this.grants.withGrant(
        platformOp({ maxDuffs: 1, maxCredits: "0" }),
        this.walletId,
        (grant) => (started(), this.api.finishTransfer(this.walletId, lock.kind, lock.txid, lock.vout, grant.id)),
        since,
      );
      if (!got) return { kind: "idle" };
      this.results.set(lockKey(lock), { lock, result: got.value });
      if (got.value.kind === "notApplied") return { kind: "failed", error: engineError(got.value.error), retry: action };
    }
    return { kind: "idle" };
  }

  #set(state: RecoveryState) {
    this.state = state;
    this.changed();
  }
}

const models = new Map<string, RecoveryModel>();

/** The wallet's model, created on first use and kept while the app runs. */
export function recoveryFor(api: RecoveryApi, walletId: string): RecoveryModel {
  let m = models.get(walletId);
  if (!m) {
    m = new RecoveryModel(api, walletId);
    models.set(walletId, m);
  }
  return m;
}

/** Tests: forget every model (a fresh app). */
export function resetRecovery() {
  models.clear();
}
