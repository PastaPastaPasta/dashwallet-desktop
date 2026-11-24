// Unfinished transfers on Home (engine `unfinished_transfers` / `finish_transfer`): Core → Shielded
// and "Add from Dash wallet" locks a wallet restore read back from the chain, which Platform is not
// known to have applied.
// The Home card's view (which copy and actions, if any), the sheet's rows, and the model that
// finishes them: one lock under a one-duff grant, or all of them with Repair's "Finish transfers".
// Only a proved apply finishes a lock; "checking" means it may already be in the balance.

import type { Backend } from "../api/backend";
import type { AppError } from "../api/generated/AppError";
import type { LockFinish } from "../api/generated/LockFinish";
import type { LockKind } from "../api/generated/LockKind";
import type { RecoverQuote } from "../api/generated/RecoverQuote";
import type { UnfinishedLock } from "../api/generated/UnfinishedLock";
import type { UnfinishedState } from "../api/generated/UnfinishedState";
import type { UnfinishedTransfers } from "../api/generated/UnfinishedTransfers";
import { creditsToDuffs } from "../api/pending";
import { REG_COPY } from "../dashpay/registration";
import { platformOp } from "../dashpay/topup";
import { finishGrant } from "../dashpay/wizard";
import { messageTable, t } from "../i18n";
import { en } from "../i18n/en";
import { Model } from "../onboarding/model";
import { onMoveSettledNotify } from "../platform/move";
import { GrantModel, PAYMENT_PROMPT } from "../tools/grant";

export type RecoveryApi = Pick<
  Backend,
  | "finishTransfer"
  | "finishAssetLocks"
  | "recoverTransferQuote"
  | "recoverTransfer"
  | "registrationQuote"
  | "authorize"
  | "revokeGrant"
  | "lockStatus"
  | "paymentAuthentication"
  | "unlock"
  | "unlockMixingOnly"
>;

/** The destinations of a set of locks: one kind, or both. */
export type LockKinds = LockKind | "mixed";

export const lockKinds = (locks: readonly { kind: LockKind }[]): LockKinds => {
  const kinds = new Set(locks.map((l) => l.kind));
  return kinds.size > 1 ? "mixed" : kinds.has("platformAddress") ? "platformAddress" : "shielded";
};

export const RECOVERY_COPY = {
  get title() {
    return t("onboarding.unfinished.title");
  },
  one: (amount: string, kind: LockKind = "shielded") =>
    kind === "platformAddress" ? t("onboarding.unfinished.oneToPlatformAddress", { amount }) : t("onboarding.unfinished.oneToShielded", { amount }),
  several: (n: number, total: string, kinds: LockKinds = "shielded") =>
    kinds === "mixed"
      ? t("onboarding.unfinished.severalMixed", { count: n, total })
      : kinds === "platformAddress"
        ? t("onboarding.unfinished.severalToPlatformAddress", { count: n, total })
        : t("onboarding.unfinished.severalToShielded", { count: n, total }),
  get running() {
    return t("onboarding.unfinished.running");
  },
  checking: (n: number) => t("onboarding.unfinished.checking", { count: n }),
  get finish() {
    return t("onboarding.unfinished.finish");
  },
  get finishAll() {
    return t("onboarding.unfinished.finishAll");
  },
  get checkAgain() {
    return t("onboarding.unfinished.checkAgain");
  },
  get details() {
    return t("onboarding.unfinished.details");
  },
  get retry() {
    return t("onboarding.unfinished.retry");
  },
  get dismiss() {
    return t("onboarding.unfinished.dismiss");
  },
  get recover() {
    return t("onboarding.unfinished.recover");
  },
  recoverOne: (amount: string) => t("onboarding.unfinished.recoverOne", { amount }),
  recoverSeveral: (n: number, total: string) => t("onboarding.unfinished.recoverSeveral", { count: n, total }),
  recoverOffOne: (amount: string) => t("onboarding.unfinished.recoverOffOne", { amount }),
  recoverOffSeveral: (n: number, total: string) => t("onboarding.unfinished.recoverOffSeveral", { count: n, total }),
  recoverConfirm: {
    get title() {
      return t("onboarding.unfinished.recoverConfirm.title");
    },
    body: (amount: string) => t("onboarding.unfinished.recoverConfirm.body", { amount }),
    get confirm() {
      return t("onboarding.unfinished.recoverConfirm.confirm");
    },
    get cancel() {
      return t("onboarding.unfinished.recoverConfirm.cancel");
    },
  },
  sheet: {
    get title() {
      return t("onboarding.unfinished.sheet.title");
    },
    intro: (kinds: LockKinds = "shielded") => (kinds === "shielded" ? t("onboarding.unfinished.sheet.introShielded") : t("onboarding.unfinished.sheet.introOther")),
    get footer() {
      return t("onboarding.unfinished.sheet.footer");
    },
    get close() {
      return t("onboarding.unfinished.sheet.close");
    },
    recoverTooSmall: (amount: string) => t("onboarding.unfinished.sheet.recoverTooSmall", { amount }),
    get recoverChecking() {
      return t("onboarding.unfinished.sheet.recoverChecking");
    },
    get recoverOff() {
      return t("onboarding.unfinished.sheet.recoverOff");
    },
    get testNetworksOnly() {
      return t("onboarding.unfinished.sheet.testNetworksOnly");
    },
    get recoverFailed() {
      return t("onboarding.unfinished.sheet.recoverFailed");
    },
    kind: messageTable("onboarding.unfinished.sheet.kind", Object.keys(en.onboarding.unfinished.sheet.kind satisfies Record<LockKind, string>) as LockKind[]),
    state: messageTable("onboarding.unfinished.sheet.state", Object.keys(en.onboarding.unfinished.sheet.state satisfies Record<UnfinishedState, string>) as UnfinishedState[]),
    done: {
      applied: (kind: LockKind = "shielded") => (kind === "platformAddress" ? t("onboarding.unfinished.sheet.done.appliedPlatformAddress") : t("onboarding.unfinished.sheet.done.appliedShielded")),
      get checking() {
        return t("onboarding.unfinished.sheet.done.checking");
      },
      get proofWaiting() {
        return t("onboarding.unfinished.sheet.done.proofWaiting");
      },
      get reportedUsed() {
        return t("onboarding.unfinished.sheet.done.reportedUsed");
      },
    },
  },
} as const;

export type CardAction = "finish" | "finishAll" | "checkAgain" | "recover" | "details" | "retry" | "dismiss";

export interface CardView {
  kind: "running" | "failed" | "ready" | "checking" | "recoverable" | "done";
  text: string;
  /** The result line of the last Finish all, shown above the rest. */
  note: string | null;
  actions: CardAction[];
  /** The lock "Finish" or "Recover" is for. */
  lock: UnfinishedLock | null;
}

type Action =
  | { kind: "finish"; lock: UnfinishedLock }
  | { kind: "all" }
  | { kind: "check"; locks: UnfinishedLock[] }
  | ({ kind: "recover" } & RecoverAsk);

/** A recover asked for: the lock and the credits Platform's proved state showed left just now. */
export interface RecoverAsk {
  lock: UnfinishedLock;
  credits: string;
}

/** What a recover of a lock came to, when it moved nothing: too small to move, or it failed. */
export type RecoverNote = { kind: "tooSmall"; credits: string } | { kind: "failed" } | { kind: "checking" } | { kind: "off" };

/** The engine's refusal of a recover on mainnet (`unfinished_remainder::FEATURE`). */
const FEATURE_OFF = "platform.feature_off";

/** A recover note as the card and the sheet say it. */
export function recoverNoteText(note: RecoverNote, duffs: (d: number) => string): string {
  switch (note.kind) {
    case "tooSmall":
      return RECOVERY_COPY.sheet.recoverTooSmall(duffs(creditsToDuffs(note.credits)));
    case "checking":
      return RECOVERY_COPY.sheet.recoverChecking;
    case "off":
      return RECOVERY_COPY.sheet.recoverOff;
    case "failed":
      return RECOVERY_COPY.sheet.recoverFailed;
  }
}

const refusalNote = (error: string): RecoverNote => (error === FEATURE_OFF ? { kind: "off" } : { kind: "failed" });

/** A transfer to Shielded Platform proved partly used: listed whatever its origin (this device's own
 * refused shield is no pending shield), and only ever recovered. */
const partlyUsed = (l: UnfinishedLock): l is UnfinishedLock & { remainingCredits: string } => l.remainingCredits !== null;

export type RecoveryState =
  | { kind: "idle" }
  | { kind: "running" }
  | { kind: "failed"; error: unknown; retry: Action }
  | { kind: "done"; text: string };

/** What happened to one lock in this session. */
export type RowResult = LockFinish;

export const lockKey = (l: Pick<UnfinishedLock, "kind" | "txid" | "vout">) => `${l.kind}:${l.txid}:${l.vout}`;

export const restoredOf = (data: UnfinishedTransfers | null | undefined): UnfinishedLock[] => data?.locks.filter((l) => l.origin === "restored" || partlyUsed(l)) ?? [];

/** `data` without the locks the card already shows as this install's pending shields. */
export function withoutPendingShields(data: UnfinishedTransfers | null | undefined, pending: readonly { txid: string }[] | null | undefined): UnfinishedTransfers | null | undefined {
  if (!data || !pending || pending.length === 0) return data;
  return { ...data, locks: data.locks.filter((l) => !pending.some((p) => p.txid === l.txid)) };
}

/** A failed finish whose locks have all left the list has nothing to retry. */
export function failureStale(state: RecoveryState, data: UnfinishedTransfers | null | undefined): boolean {
  if (state.kind !== "failed" || !data) return false;
  const locks = state.retry.kind === "finish" ? [state.retry.lock] : state.retry.kind === "check" ? state.retry.locks : null;
  return locks !== null && !locks.some((l) => data.locks.some((x) => lockKey(x) === lockKey(l)));
}

/** The Home card, `null` for none. It shows when the list is read and complete and holds a restored
 * lock that is ready or checking, and while a finish from the card runs, failed or reported. Its
 * Finish all (`finish_asset_locks`) tries the ready locks only; checking ones keep Check again. */
export function cardView(
  data: UnfinishedTransfers | null | undefined,
  state: RecoveryState,
  duffs: (d: number) => string,
  errorText: (e: unknown) => string,
  mainnet = false,
): CardView | null {
  if (state.kind === "running") return { kind: "running", text: RECOVERY_COPY.running, note: null, actions: [], lock: null };
  if (state.kind === "failed" && !failureStale(state, data)) return { kind: "failed", text: errorText(state.error), note: null, actions: ["retry", "dismiss"], lock: null };
  const note = state.kind === "done" ? state.text : null;
  const restored = data?.complete ? restoredOf(data) : [];
  const ready = restored.filter((l) => l.state === "ready");
  const checking = restored.filter((l) => l.state === "checking");
  const details: CardAction[] = restored.length > 1 ? ["details"] : [];
  if (ready.length === 1) return { kind: "ready", text: RECOVERY_COPY.one(duffs(ready[0].duffs), ready[0].kind), note, actions: ["finish", ...details], lock: ready[0] };
  if (ready.length > 1) {
    const total = ready.reduce((n, l) => n + l.duffs, 0);
    return { kind: "ready", text: RECOVERY_COPY.several(ready.length, duffs(total), lockKinds(ready)), note, actions: ["finishAll", "details"], lock: null };
  }
  if (checking.length > 0) return { kind: "checking", text: RECOVERY_COPY.checking(checking.length), note, actions: ["checkAgain", "details"], lock: null };
  const recoverable = restored.filter(partlyUsed).filter((l) => l.state === "recoverable");
  if (recoverable.length === 1) {
    const [lock] = recoverable;
    const amount = duffs(creditsToDuffs(lock.remainingCredits));
    // Off on mainnet in this version: no Recover anywhere.
    if (mainnet) return { kind: "recoverable", text: RECOVERY_COPY.recoverOffOne(amount), note, actions: details, lock: null };
    return { kind: "recoverable", text: RECOVERY_COPY.recoverOne(amount), note, actions: ["recover", ...details], lock };
  }
  if (recoverable.length > 1) {
    const total = duffs(creditsToDuffs(String(recoverable.reduce((n, l) => n + BigInt(l.remainingCredits), 0n))));
    const text = mainnet ? RECOVERY_COPY.recoverOffSeveral(recoverable.length, total) : RECOVERY_COPY.recoverSeveral(recoverable.length, total);
    return { kind: "recoverable", text, note, actions: ["details"], lock: null };
  }
  return note === null ? null : { kind: "done", text: note, note: null, actions: ["dismiss"], lock: null };
}

/** The sheet's rows: the restored locks, and the ones this session applied or Platform reported already
 * added (they have left the list). */
export function sheetRows(data: UnfinishedTransfers | null | undefined, results: ReadonlyMap<string, { lock: UnfinishedLock; result: RowResult }>) {
  const rows = restoredOf(data).map((lock) => ({ lock, result: results.get(lockKey(lock))?.result ?? null }));
  const gone = [...results.values()].filter((r) => (r.result.kind === "applied" || r.result.kind === "reportedUsed") && !rows.some((x) => lockKey(x.lock) === lockKey(r.lock)));
  return [...rows, ...gone.map((r) => ({ lock: r.lock, result: r.result as RowResult | null }))];
}

/** A row's done line; `kind` is the lock's. */
export function resultText(r: RowResult, errorText: (e: unknown) => string, kind: LockKind = "shielded"): string {
  switch (r.kind) {
    case "applied":
      return RECOVERY_COPY.sheet.done.applied(kind);
    case "checking":
      return RECOVERY_COPY.sheet.done.checking;
    case "reportedUsed":
      return RECOVERY_COPY.sheet.done.reportedUsed;
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

const notifySettled = () => {
  for (const f of settledListeners) queueMicrotask(f);
};

export class RecoveryModel extends Model {
  state: RecoveryState = { kind: "idle" };
  /** Per lock: what its last finish came to in this session (`recovered`: a recover's). */
  results = new Map<string, { lock: UnfinishedLock; result: RowResult; recovered?: boolean }>();
  /** Per lock: a recover that moved nothing. */
  notes = new Map<string, RecoverNote>();
  /** The recover waiting for the user's confirmation. */
  asking: RecoverAsk | null = null;
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

  /** Reads what is left on a partly used lock from Platform's proved state now; enough to move asks
   * the user to confirm (`asking`). `duffs` formats an amount for the card's line. */
  async askRecover(lock: UnfinishedLock, duffs: (d: number) => string): Promise<void> {
    if (this.running) return;
    const key = lockKey(lock);
    this.notes.delete(key);
    this.#set({ kind: "running" });
    let quote: RecoverQuote | null = null;
    try {
      quote = await this.api.recoverTransferQuote(this.walletId, lock.txid, lock.vout);
    } catch {
      quote = null;
    }
    if (quote?.kind === "ready") {
      this.asking = { lock, credits: quote.credits };
      return this.#set({ kind: "idle" });
    }
    if (quote?.kind === "finished") {
      // Recovered, or used up elsewhere: the list read again drops it.
      notifySettled();
      return this.#set({ kind: "idle" });
    }
    const note: RecoverNote = quote?.kind === "tooSmall" ? { kind: "tooSmall", credits: quote.credits } : quote?.kind === "notApplied" ? refusalNote(quote.error) : { kind: "failed" };
    this.notes.set(key, note);
    // The quote may have changed what is listed (a fresh too-small value).
    if (note.kind === "tooSmall") notifySettled();
    this.#set({ kind: "done", text: recoverNoteText(note, duffs) });
  }

  /** The user confirmed: the recover runs under a grant of exactly the quoted credits. */
  confirmRecover(): Promise<void> {
    const ask = this.asking;
    this.asking = null;
    return ask ? this.#run({ kind: "recover", lock: ask.lock, credits: ask.credits }) : Promise.resolve();
  }

  cancelRecover() {
    this.asking = null;
    this.changed();
  }

  /** Drops a failure whose locks are no longer listed. */
  settle(data: UnfinishedTransfers | null | undefined) {
    if (failureStale(this.state, data)) this.#set({ kind: "idle" });
  }

  dismiss() {
    if (this.state.kind === "done" || this.state.kind === "failed") this.#set({ kind: "idle" });
  }

  async #run(action: Action) {
    if (this.running) return;
    const since = this.grants.epoch;
    this.#set({ kind: "running" });
    let ran = false;
    let platform = false;
    try {
      const next = await this.#do(action, since, () => (ran = true), () => (platform = true));
      this.#set(next);
    } catch (e) {
      this.#set({ kind: "failed", error: e, retry: action });
    } finally {
      if (ran) notifySettled();
      if (ran && platform) onMoveSettledNotify();
    }
  }

  async #do(action: Action, since: number, started: () => void, touchedPlatform: () => void): Promise<RecoveryState> {
    if (action.kind === "all") {
      const caps = await finishGrant(this.api, this.walletId);
      const report = await this.grants.withGrant(
        { kind: "platformOp", maxDuffs: caps.maxDuffs, maxCredits: caps.maxCredits },
        this.walletId,
        (grant) => (started(), this.api.finishAssetLocks(this.walletId, grant.id)),
        since,
      );
      if (report?.value.byKind.some((k) => k.kind === "platformAddress")) touchedPlatform();
      return report ? { kind: "done", text: REG_COPY.finished(report.value) } : { kind: "idle" };
    }
    if (action.kind === "recover") return this.#recover(action, since, started, touchedPlatform);
    const locks = action.kind === "finish" ? [action.lock] : action.locks;
    for (const lock of locks) {
      if (lock.kind === "platformAddress") touchedPlatform();
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
    const only = locks.length === 1 ? this.results.get(lockKey(locks[0]))?.result : undefined;
    return only?.kind === "reportedUsed" ? { kind: "done", text: RECOVERY_COPY.sheet.done.reportedUsed } : { kind: "idle" };
  }

  async #recover({ lock, credits }: RecoverAsk, since: number, started: () => void, touchedPlatform: () => void): Promise<RecoveryState> {
    const key = lockKey(lock);
    try {
      const got = await this.grants.withGrant(
        platformOp({ maxDuffs: 1, maxCredits: credits }),
        this.walletId,
        (grant) => (started(), touchedPlatform(), this.api.recoverTransfer(this.walletId, lock.txid, lock.vout, grant.id)),
        since,
      );
      if (!got) return { kind: "idle" };
      if (got.value.kind === "applied" || got.value.kind === "reportedUsed") {
        this.results.set(key, { lock, result: got.value, recovered: true });
        return { kind: "idle" };
      }
      // Checking may have gone through; a refusal before the signature did not.
      const note: RecoverNote = got.value.kind === "notApplied" ? refusalNote(got.value.error) : { kind: "checking" };
      this.notes.set(key, note);
      return { kind: "done", text: recoverNoteText(note, String) };
    } catch {
      this.notes.set(key, { kind: "failed" });
      return { kind: "done", text: RECOVERY_COPY.sheet.recoverFailed };
    }
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
