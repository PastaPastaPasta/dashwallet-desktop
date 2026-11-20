// A proposal's fee payment whose outcome the page has to answer for: a lock takes the page down
// at once, and `create_proposal` may already have paid the fee when its answer is lost. Calling it
// again for the same draft pays a second fee. The engine names a proposal only once its call
// answers (its hash leaves out the fee's transaction but covers the time it was made, which the
// renderer does not know), so an attempt is known by what the user entered. The rules:
//
// 1. Pay is dispatched only after a Mine read that began after every earlier attempt of the
//    wallet was recorded, succeeded, and belongs to the current lock generation (`beginCreate`).
//    That read reconciles the ledger, the hold is checked against it, and its matching proposals
//    are the new attempt's baseline. No successful read, no Pay.
// 2. Every Mine observation carries its request's sequence and lock generation (`readMine`). One
//    older than the newest applied, or from another generation, is dropped. A proposal that more
//    than one attempt could have made, or one of several an attempt could have made, marks it
//    uncertain; only an attempt's own answer clears that, and the user acknowledging the attempt
//    stops it holding Create. A later list never does.
// 3. Results and acknowledgements go by attempt: a sheet reads its own (`attemptStatus`), and a
//    button acknowledges the attempts it shows (`acknowledge`).
// 4. The ledger keeps what outstanding attempts need (their drafts and proposal hashes), never
//    Mine's rows. A removed wallet or vault forgets its attempts and its reads still out
//    (`forgetKeptCreate`), and nothing answered after brings them back.
// 5. A draft matches a proposal exactly: amount in duffs, payment count, and the first payment's
//    epoch within a quarter cycle of the preview's. What still collides is uncertain, not made.
// 6. An attempt whose outcome is not known holds Create, with a stated way on: a different
//    proposal (`goOnWithAnother`). Its draft stays held and its notice stays.
//
// Only the page draws this, and it is not mounted while locked. Quitting forgets it.

import { useSyncExternalStore } from "react";
import type { MyProposal } from "../../api/generated/MyProposal";
import type { ProposalDraft } from "../../api/generated/ProposalDraft";
import type { ProposalPreview } from "../../api/generated/ProposalPreview";
import { lockGeneration, subscribeWalletGate, walletGateClosed } from "../../api/lockGate";
import { GovText, inProgress } from "../../governance/model";

/** What a proposal in Mine shows of the draft that made it, with the schedule its preview gave. */
export type ProposalPrint = Pick<ProposalDraft, "name" | "url" | "paymentAddress" | "paymentAmount" | "paymentCount"> &
  Pick<ProposalPreview, "startEpoch" | "endEpoch">;

type Phase = { kind: "paying" } | { kind: "unsure"; since: number; checked: number | null; failed: boolean } | { kind: "made"; hash: string; wasUnsure: boolean };

type Attempt = {
  id: number;
  print: ProposalPrint;
  /** Proposals of this draft Mine held when Pay was pressed: never this attempt's. */
  baseline: readonly string[];
  /** The sequence at which it was recorded. */
  recorded: number;
  phase: Phase;
  /** Proposals this attempt may have made, each with the other attempts that may have made it. */
  uncertain: Map<string, Set<number>>;
  /** The status the user acknowledged: seen until it changes. */
  seenAs: AttemptStatus | null;
  /** The user went on with another proposal while its outcome is not known. */
  released: boolean;
};

export type AttemptStatus = "paying" | "reading" | "failed" | "absent" | "uncertain" | "found" | "created";

/** What the page shows: the most pressing attempt and the attempts its notice speaks for, and why
 * Create waits, over all of the wallet's attempts. */
export type KeptCreate = {
  status: AttemptStatus;
  name: string;
  ids: number[];
  hash: string | null;
  /** Some of `ids` can be acknowledged; the rest wait for their outcome. */
  dismissable: boolean;
  /** Some of `ids` are not known and the user has not gone on with another proposal. */
  waiting: boolean;
  held: string | null;
} | null;

let sequence = 0;
let attemptIds = 0;
/** Bumped when every wallet is forgotten: what was asked before answers nothing. */
let vault = 0;
const ledgers = new Map<string, Attempt[]>();
/** The newest observation applied, per wallet. */
const observed = new Map<string, number>();
/** Bumped when a wallet is forgotten. */
const incarnation = new Map<string, number>();
/** Each wallet's view, with the lock generation it was drawn in: a check from before is stale. */
const views = new Map<string, { generation: number; view: KeptCreate }>();
const reading = new Set<string>();
const listeners = new Set<() => void>();
const subscribe = (l: () => void) => {
  listeners.add(l);
  return () => void listeners.delete(l);
};

const bornAs = (walletId: string) => `${vault}:${incarnation.get(walletId) ?? 0}`;

function statusOf(a: Attempt): AttemptStatus {
  if (a.phase.kind === "made") return a.phase.wasUnsure ? "found" : "created";
  if (a.uncertain.size) return "uncertain";
  if (a.phase.kind === "paying") return "paying";
  if (a.phase.failed) return "failed";
  return a.phase.checked === lockGeneration() ? "absent" : "reading";
}

const seen = (a: Attempt) => a.seenAs === statusOf(a);
/** Its outcome is not known: its call has not answered, or Mine has not said since. It holds
 * Create until the user goes on with another proposal, and cannot be acknowledged. */
const pending = (a: Attempt) => a.phase.kind === "paying" || statusOf(a) === "reading" || statusOf(a) === "failed";
const ORDER: readonly AttemptStatus[] = ["failed", "reading", "paying", "uncertain", "found", "created", "absent"];
const shows = (a: Attempt) => pending(a) || !seen(a);
/** A second payment for `a`'s draft now may pay a second fee. */
const atRisk = (a: Attempt) => pending(a) || (!seen(a) && (statusOf(a) === "uncertain" || statusOf(a) === "found"));

function heldBy(a: Attempt): string | null {
  const s = statusOf(a);
  if (!seen(a) && s === "uncertain") return GovText.createTwin(a.print.name);
  if (!seen(a) && s === "found") return GovText.createFound(a.print.name);
  if (!pending(a) || a.released) return null;
  return a.phase.kind === "paying" ? GovText.keptPaying : s === "failed" ? GovText.createReadFailed : GovText.createChecking;
}

/** Attempts sharing an uncertain proposal with `a`, `a` among them. */
function group(attempts: Attempt[], a: Attempt): Attempt[] {
  const out = new Set([a]);
  for (let grew = true; grew; ) {
    grew = false;
    for (const x of attempts)
      if (!out.has(x) && [...out].some((y) => [...y.uncertain.keys()].some((h) => x.uncertain.has(h)))) {
        out.add(x);
        grew = true;
      }
  }
  return [...out];
}

/** The notice first shown: what holds Create, an outcome not known before one told, then the
 * rest; within each, by `ORDER`. */
const rank = (a: Attempt) => (heldBy(a) !== null ? 0 : pending(a) ? 1 : 2) * ORDER.length + ORDER.indexOf(statusOf(a));

function view(attempts: Attempt[]): KeptCreate {
  const a = attempts.filter(shows).sort((x, y) => rank(x) - rank(y))[0];
  if (!a) return null;
  const s = statusOf(a);
  const ids = s === "uncertain" ? group(attempts, a).filter(shows).map((x) => x.id) : [a.id];
  const shown = attempts.filter((x) => ids.includes(x.id));
  return {
    status: s,
    name: a.print.name,
    ids,
    hash: a.phase.kind === "made" ? a.phase.hash : null,
    dismissable: shown.some((x) => !pending(x)),
    waiting: shown.some((x) => pending(x) && !x.released),
    held: heldBy(a),
  };
}

function publish(walletId: string) {
  views.set(walletId, { generation: lockGeneration(), view: view(ledgers.get(walletId) ?? []) });
  listeners.forEach((l) => l());
}

export function keptCreate(walletId: string): KeptCreate {
  const v = views.get(walletId);
  if (!v) return null;
  if (v.generation !== lockGeneration()) views.set(walletId, { generation: lockGeneration(), view: view(ledgers.get(walletId) ?? []) });
  return views.get(walletId)!.view;
}

// A lock or an unlock moves the generation: what was read before is drawn as being read again.
subscribeWalletGate(() => listeners.forEach((l) => l()));

export const useKeptCreate = (walletId: string): KeptCreate => useSyncExternalStore(subscribe, () => keptCreate(walletId));

/** `p` is a proposal of `print`: every field the user entered, and the first payment's epoch
 * within a quarter cycle of the preview's (the engine dates it from the tip when it is made). */
export function matches(print: ProposalPrint, p: MyProposal): boolean {
  const cycle = print.paymentCount > 0 ? (print.endEpoch - print.startEpoch) / print.paymentCount : 0;
  return (
    p.name === print.name &&
    p.url === print.url &&
    p.paymentAddress === print.paymentAddress &&
    p.paymentAmount === print.paymentAmount &&
    p.paymentCount === print.paymentCount &&
    (cycle > 0 ? Math.abs(p.startEpoch - print.startEpoch) < cycle / 4 : p.startEpoch === print.startEpoch)
  );
}

const open = (a: Attempt) => a.phase.kind !== "made";

/** Applies a successful Mine read that began at `seq` in lock generation `generation` (rule 2). */
function observe(walletId: string, seq: number, generation: number, rows: readonly MyProposal[]): boolean {
  if (seq <= (observed.get(walletId) ?? 0) || generation !== lockGeneration() || walletGateClosed()) return false;
  observed.set(walletId, seq);
  const attempts = ledgers.get(walletId);
  if (!attempts?.length) return true;
  for (const a of attempts) if (a.phase.kind === "unsure" && a.phase.since < seq) a.phase = { ...a.phase, checked: generation, failed: false };
  const claimed = new Set(attempts.flatMap((a) => (a.phase.kind === "made" ? [a.phase.hash] : [])));
  const live = rows.filter((p) => inProgress(p) && !claimed.has(p.hash));
  const candidates = new Map<Attempt, MyProposal[]>();
  for (const a of attempts) if (open(a) && a.recorded < seq) candidates.set(a, live.filter((p) => matches(a.print, p) && !a.baseline.includes(p.hash)));
  const owners = (p: MyProposal) => [...candidates].filter(([, ps]) => ps.includes(p)).map(([a]) => a);
  for (const [a, ps] of candidates) {
    const sole = ps.length === 1 && owners(ps[0]).length === 1;
    if (sole && !a.uncertain.size) {
      a.phase = { kind: "made", hash: ps[0].hash, wasUnsure: a.phase.kind === "unsure" };
      continue;
    }
    if (sole) continue;
    for (const p of ps) {
      const rivals = a.uncertain.get(p.hash) ?? new Set<number>();
      for (const o of owners(p)) if (o !== a) rivals.add(o.id);
      a.uncertain.set(p.hash, rivals);
    }
  }
  publish(walletId);
  return true;
}

export type MineRead = { rows: MyProposal[]; applied: boolean; seq: number };

/** Reads Mine for `walletId` and applies it in order (rule 2). Rejects when the read failed;
 * `applied` is false when it can no longer count. Nothing of the rows is kept. */
export async function readMine(walletId: string, read: () => Promise<MyProposal[]>): Promise<MineRead> {
  const seq = ++sequence;
  const generation = lockGeneration();
  const born = bornAs(walletId);
  const rows = await read();
  return { rows, applied: born === bornAs(walletId) && observe(walletId, seq, generation, rows), seq };
}

export type BeginCreate = { kind: "go"; attempt: number } | { kind: "held"; reason: string } | { kind: "unread" };

/** Rule 1: reads Mine, then in one step after it answers reconciles the ledger with it, checks
 * the holds against it and records the attempt with it as the baseline. */
export async function beginCreate(walletId: string, print: ProposalPrint, read: () => Promise<MyProposal[]>): Promise<BeginCreate> {
  for (let tries = 0; tries < 3; tries++) {
    const generation = lockGeneration();
    let r: MineRead;
    try {
      r = await readMine(walletId, read);
    } catch {
      return { kind: "unread" };
    }
    if (walletGateClosed() || generation !== lockGeneration()) return { kind: "unread" };
    // A newer read applied, or another attempt recorded, since this read began: read again.
    const attempts = ledgers.get(walletId) ?? [];
    if (!r.applied || observed.get(walletId) !== r.seq || attempts.some((a) => a.recorded > r.seq)) continue;
    if (attempts.some((a) => a.print.name === print.name && atRisk(a))) return { kind: "held", reason: GovText.sameDraftHeld(print.name) };
    const held = view(attempts)?.held;
    if (held) return { kind: "held", reason: held };
    const attempt: Attempt = {
      id: ++attemptIds,
      print: { ...print },
      baseline: r.rows.filter((p) => matches(print, p)).map((p) => p.hash),
      recorded: ++sequence,
      phase: { kind: "paying" },
      uncertain: new Map(),
      seenAs: null,
      released: false,
    };
    ledgers.set(walletId, [...attempts, attempt]);
    publish(walletId);
    return { kind: "go", attempt: attempt.id };
  }
  return { kind: "unread" };
}

const find = (walletId: string, id: number) => ledgers.get(walletId)?.find((a) => a.id === id);

/** Attempt `id`'s call answered: `created` with the proposal it names (`shown` when its sheet
 * showed it), `unsure` when the fee may have gone out, or `null` when it was never paid. */
export function settleCreate(walletId: string, id: number, outcome: { kind: "created"; hash: string; shown: boolean } | { kind: "unsure" } | null) {
  const a = find(walletId, id);
  if (!a) return;
  const attempts = ledgers.get(walletId)!;
  if (outcome === null) {
    if (a.phase.kind === "paying") ledgers.set(walletId, attempts.filter((x) => x !== a));
  } else if (outcome.kind === "created") {
    // Its own answer: the proposal is this attempt's, and no other attempt made it.
    a.phase = { kind: "made", hash: outcome.hash, wasUnsure: false };
    a.uncertain.clear();
    for (const x of attempts)
      for (const [h, rivals] of x.uncertain)
        if (h === outcome.hash) x.uncertain.delete(h);
        else if (rivals.delete(a.id) && rivals.size === 0) x.uncertain.delete(h);
    if (outcome.shown) a.seenAs = statusOf(a);
  } else if (a.phase.kind === "paying") a.phase = { kind: "unsure", since: ++sequence, checked: null, failed: false };
  // Mine showed it while the call was out, and the call failed: told as found, not as made.
  else if (a.phase.kind === "made") a.phase = { ...a.phase, wasUnsure: true };
  trim(walletId);
  publish(walletId);
}

/** What the sheet of attempt `id` shows: its own outcome, never another attempt's. */
export function attemptStatus(walletId: string, id: number): { status: AttemptStatus; hash: string | null } | null {
  const a = find(walletId, id);
  return a ? { status: statusOf(a), hash: a.phase.kind === "made" ? a.phase.hash : null } : null;
}

/** An earlier payment for a draft named `name` may have paid a fee: the sheet says so. */
export function priorRisk(walletId: string, name: string): string | null {
  const earlier = (ledgers.get(walletId) ?? []).filter((a) => a.print.name === name);
  return earlier.some((a) => a.phase.kind === "unsure" || a.uncertain.size || (a.phase.kind === "made" && a.phase.wasUnsure)) ? GovText.priorFee(name) : null;
}

/** Reads Mine when an attempt whose call failed has no read in this lock generation. */
export async function checkKept(walletId: string, read: () => Promise<MyProposal[]>) {
  if (!(ledgers.get(walletId) ?? []).some((a) => statusOf(a) === "reading") || reading.has(walletId) || walletGateClosed()) return;
  const born = bornAs(walletId);
  const seq = sequence + 1;
  reading.add(walletId);
  try {
    await readMine(walletId, read);
  } catch {
    if (born === bornAs(walletId))
      for (const a of ledgers.get(walletId) ?? [])
        if (a.phase.kind === "unsure" && a.phase.since < seq && a.phase.checked !== lockGeneration()) a.phase = { ...a.phase, failed: true };
  } finally {
    if (born === bornAs(walletId)) {
      reading.delete(walletId);
      publish(walletId);
    }
  }
}

/** Read again: Mine could not be read. */
export function readKeptAgain(walletId: string) {
  for (const a of ledgers.get(walletId) ?? []) if (a.phase.kind === "unsure" && a.phase.failed) a.phase = { ...a.phase, failed: false };
  publish(walletId);
}

/** The user saw what attempts `ids` say. An outcome not known yet stays. */
export function acknowledge(walletId: string, ids: readonly number[]) {
  for (const a of ledgers.get(walletId) ?? []) if (ids.includes(a.id) && !pending(a)) a.seenAs = statusOf(a);
  trim(walletId);
  publish(walletId);
}

/** Rule 6: the user goes on with a different proposal while attempts `ids` are not known. */
export function goOnWithAnother(walletId: string, ids: readonly number[]) {
  for (const a of ledgers.get(walletId) ?? []) if (ids.includes(a.id)) a.released = true;
  publish(walletId);
}

/** Drops attempts with nothing left to tell or to guard. */
function trim(walletId: string) {
  const attempts = ledgers.get(walletId) ?? [];
  const keep = attempts.filter((a) => open(a) || a.uncertain.size || !seen(a) || attempts.some((x) => x !== a && open(x) && x.print.name === a.print.name));
  if (keep.length) ledgers.set(walletId, keep);
  else ledgers.delete(walletId);
}

/** Rule 4: a removed wallet, or every wallet when the vault goes, with its reads still out. */
export function forgetKeptCreate(walletId?: string) {
  if (walletId === undefined) {
    vault++;
    ledgers.clear();
    observed.clear();
    reading.clear();
    views.clear();
  } else {
    incarnation.set(walletId, (incarnation.get(walletId) ?? 0) + 1);
    ledgers.delete(walletId);
    observed.delete(walletId);
    reading.delete(walletId);
    views.delete(walletId);
  }
  listeners.forEach((l) => l());
}

/** Tests: whether anything is kept for `walletId`. */
export const keepsAnything = (walletId: string): boolean => ledgers.has(walletId) || observed.has(walletId) || reading.has(walletId) || views.has(walletId);

/** Tests: what the ledger holds for `walletId`, as text. */
export const ledgerSnapshot = (walletId: string): string =>
  JSON.stringify(ledgers.get(walletId) ?? [], (_, v: unknown) => (v instanceof Map ? [...v] : v instanceof Set ? [...v] : v));

/** Tests: nothing kept. */
export function resetKeptCreate() {
  forgetKeptCreate();
  incarnation.clear();
}
