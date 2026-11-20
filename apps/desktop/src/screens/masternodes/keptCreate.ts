// A proposal's fee payment that outlived its sheet, or whose outcome is not known yet: a lock
// takes the page down at once, and `create_proposal` may already have paid the fee. Calling it
// again with the same draft pays a second fee, so Create (and the fee call itself) waits until
// Mine, read since the unlock, says whether the attempt made its proposal. Only the page draws
// this, and the page is not mounted while locked.
//
// The engine names a proposal only once its call answers (its hash covers the fee and the time),
// so an attempt is known by everything the user entered. A row in Mine is credited to an attempt
// only when no other open attempt could have made it; when two could, both fees may have gone out
// and the notice says so rather than pick one.

import { useSyncExternalStore } from "react";
import type { MyProposal } from "../../api/generated/MyProposal";
import type { ProposalDraft } from "../../api/generated/ProposalDraft";
import { lockGeneration, walletGateClosed } from "../../api/lockGate";
import { GovText, inProgress } from "../../governance/model";

/** What a proposal in Mine shows of the draft that made it. */
export type ProposalPrint = Pick<ProposalDraft, "name" | "url" | "paymentAddress" | "paymentAmount" | "paymentCount">;

/** One press of Pay: the draft as entered, and the proposals Mine held as it was pressed. */
export type CreateAttempt = { id: number; name: string; print: ProposalPrint; known: readonly string[] };

/** What Mine, read since the unlock, said of an attempt whose call failed. `twin`: Mine shows a
 * proposal that this attempt or another one of the same draft made, and which is not known. */
export type MineCheck = { kind: "reading" } | { kind: "found"; p: MyProposal } | { kind: "absent" } | { kind: "failed" } | { kind: "twin" };

/** What the page shows of the wallet's attempts: the one that matters most. */
export type KeptCreate =
  | { kind: "paying"; attempt: CreateAttempt }
  | { kind: "created"; p: MyProposal }
  | { kind: "unsure"; attempt: CreateAttempt; check: MineCheck };

type State = { kind: "paying" } | { kind: "unsure"; read: "reading" | "failed" | "done" } | { kind: "made"; p: MyProposal; wasUnsure: boolean };

type Entry = {
  attempt: CreateAttempt;
  state: State;
  /** A proposal in Mine is this attempt's or another open one's. */
  twin: boolean;
  /** What it said when the user last saw it: it is seen until that changes. */
  seenAs: string | null;
};

/** Attempts kept per wallet beyond the open ones. */
const MAX_ENTRIES = 32;

const ledger = new Map<string, Entry[]>();
/** Mine as last read, to settle against once an attempt's own answer names its proposal. */
const lastMine = new Map<string, readonly MyProposal[]>();
const views = new Map<string, KeptCreate | null>();
const reading = new Set<string>();
const listeners = new Set<() => void>();
const subscribe = (l: () => void) => {
  listeners.add(l);
  return () => void listeners.delete(l);
};

const entriesOf = (walletId: string): Entry[] => ledger.get(walletId) ?? [];
const open = (e: Entry) => e.state.kind !== "made";
const seen = (e: Entry) => e.seenAs === status(e);

/** What an attempt says. Only one whose call failed and whose Mine was read can be a twin: one
 * still paying, or not read yet, holds Create back anyway. */
function status(e: Entry): string {
  if (e.state.kind === "made") return e.state.wasUnsure ? "found" : "created";
  if (e.state.kind === "paying") return "paying";
  if (e.state.read !== "done") return e.state.read;
  return e.twin ? "twin" : "done";
}

function view(entries: Entry[]): KeptCreate | null {
  const first = (s: string, unseen = true) => entries.find((e) => status(e) === s && !(unseen && seen(e)));
  const failed = first("failed", false) ?? first("reading", false);
  if (failed?.state.kind === "unsure") return { kind: "unsure", attempt: failed.attempt, check: { kind: failed.state.read === "failed" ? "failed" : "reading" } };
  const twin = first("twin");
  if (twin) return { kind: "unsure", attempt: twin.attempt, check: { kind: "twin" } };
  const paying = first("paying", false);
  if (paying) return { kind: "paying", attempt: paying.attempt };
  const created = first("created");
  if (created?.state.kind === "made") return { kind: "created", p: created.state.p };
  const found = first("found");
  if (found?.state.kind === "made") return { kind: "unsure", attempt: found.attempt, check: { kind: "found", p: found.state.p } };
  const absent = first("done");
  if (absent) return { kind: "unsure", attempt: absent.attempt, check: { kind: "absent" } };
  return null;
}

const key = (e: Entry) => `${e.attempt.id}:${status(e)}:${e.state.kind === "made" ? e.state.p.hash : ""}:${e.seenAs ?? ""}`;

/** Runs `change` on copies of the wallet's attempts, and tells the page when anything it shows
 * may have changed (`force`: whether or not). */
function update(walletId: string, change: (entries: Entry[]) => Entry[], force = false) {
  const before = entriesOf(walletId);
  const after = change(before.map((e) => ({ ...e })));
  if (!force && after.map(key).join() === before.map(key).join()) return;
  if (after.length) ledger.set(walletId, after);
  else ledger.delete(walletId);
  views.set(walletId, view(after));
  listeners.forEach((l) => l());
}

export const keptCreate = (walletId: string): KeptCreate | null => views.get(walletId) ?? null;

export const useKeptCreate = (walletId: string): KeptCreate | null => useSyncExternalStore(subscribe, () => keptCreate(walletId));

let attempts = 0;

const print = (d: ProposalPrint): ProposalPrint => ({ name: d.name, url: d.url, paymentAddress: d.paymentAddress, paymentAmount: d.paymentAmount, paymentCount: d.paymentCount });

/** Pay was pressed: until the call settles, Create waits. */
export function startCreate(walletId: string, draft: ProposalPrint, mine: readonly MyProposal[]): CreateAttempt {
  const attempt = { id: ++attempts, name: draft.name, print: print(draft), known: mine.map((p) => p.hash) };
  update(walletId, (entries) => trim([...entries, { attempt, state: { kind: "paying" }, twin: false, seenAs: null }]));
  return attempt;
}

/** Keeps every open attempt; of the rest, the newest. */
function trim(entries: Entry[]): Entry[] {
  let over = entries.length - MAX_ENTRIES;
  return over <= 0 ? entries : entries.filter((e) => open(e) || !seen(e) || over-- <= 0);
}

/** `p` is a proposal `attempt` could have made: what the user entered, and new since Pay. The
 * amount went through the proposal's JSON, so it may round by a duff. */
export const couldBe = (attempt: CreateAttempt, p: MyProposal): boolean =>
  inProgress(p) &&
  !attempt.known.includes(p.hash) &&
  p.name === attempt.print.name &&
  p.url === attempt.print.url &&
  p.paymentAddress === attempt.print.paymentAddress &&
  Math.abs(p.paymentAmount - attempt.print.paymentAmount) <= 1 &&
  p.paymentCount === attempt.print.paymentCount;

/** Credits each proposal in `mine` that only one open attempt could have made to that attempt,
 * and marks the open attempts a proposal could belong to with another. An attempt whose own read
 * since the unlock has not answered yet is never credited. */
function credit(entries: Entry[], mine: readonly MyProposal[]): Entry[] {
  const claimed = new Set(entries.flatMap((e) => (e.state.kind === "made" ? [e.state.p.hash] : [])));
  let rows = mine.filter((p) => !claimed.has(p.hash));
  for (const e of entries) e.twin = false;
  const owners = (p: MyProposal) => entries.filter((e) => open(e) && couldBe(e.attempt, p));
  for (let found = true; found; ) {
    found = false;
    for (const p of rows) {
      const can = owners(p);
      const e = can[0];
      if (can.length !== 1 || (e.state.kind === "unsure" && e.state.read === "reading")) continue;
      e.state = { kind: "made", p, wasUnsure: e.state.kind === "unsure" };
      rows = rows.filter((r) => r !== p);
      found = true;
      break;
    }
  }
  for (const p of rows) {
    const can = owners(p);
    if (can.length > 1) for (const e of can) e.twin = true;
  }
  return entries;
}

/** The call settled: `created` with its proposal (`shown` when its sheet showed it), `unsure`
 * when the fee may have gone out, or `null` when it was never paid. Ignored once another
 * attempt took the wallet's place. */
export function settleCreate(walletId: string, attempt: CreateAttempt, outcome: { kind: "created"; p: MyProposal; shown: boolean } | { kind: "unsure" } | null) {
  update(walletId, (entries) => {
    const e = entries.find((x) => x.attempt.id === attempt.id);
    if (!e) return entries;
    if (outcome === null) return e.state.kind === "paying" ? entries.filter((x) => x !== e) : entries;
    if (outcome.kind === "created") {
      // Its own answer names it, whatever Mine was credited with.
      e.state = { kind: "made", p: outcome.p, wasUnsure: e.state.kind === "made" && e.state.wasUnsure };
      if (outcome.shown) e.seenAs = status(e);
      const mine = lastMine.get(walletId);
      return mine ? credit(entries, mine) : entries;
    }
    if (e.state.kind === "paying") e.state = { kind: "unsure", read: "reading" };
    return entries;
  });
}

/** Mine as the page read it. */
export function sawMine(walletId: string, mine: readonly MyProposal[]) {
  lastMine.set(walletId, mine);
  if (entriesOf(walletId).length) update(walletId, (entries) => credit(entries, mine));
}

/** Reads Mine for the attempts whose call failed, while the window is unlocked. A read the vault
 * locked during began before that lock, so it is read again; an attempt that failed during the
 * read waits for the next one. */
export async function checkKept(walletId: string, read: () => Promise<MyProposal[]>) {
  const waiting = entriesOf(walletId).filter((e) => e.state.kind === "unsure" && e.state.read === "reading");
  if (!waiting.length || reading.has(walletId) || walletGateClosed()) return;
  const ids = new Set(waiting.map((e) => e.attempt.id));
  const generation = lockGeneration();
  reading.add(walletId);
  let mine: MyProposal[] | null = null;
  try {
    mine = await read();
  } catch {
    mine = null;
  } finally {
    reading.delete(walletId);
  }
  if (walletGateClosed() || lockGeneration() !== generation) {
    update(walletId, (entries) => entries, true);
    return;
  }
  if (mine) lastMine.set(walletId, mine);
  update(walletId, (entries) => {
    for (const e of entries) if (ids.has(e.attempt.id) && e.state.kind === "unsure" && e.state.read === "reading") e.state = { kind: "unsure", read: mine ? "done" : "failed" };
    return mine ? credit(entries, mine) : entries;
  });
}

/** The page showed a made proposal. */
export function keepCreateDone(walletId: string) {
  update(walletId, (entries) => entries.map((e) => (status(e) === "created" ? { ...e, seenAs: "created" } : e)));
}

/** Read again: Mine could not be read. */
export function readKeptAgain(walletId: string) {
  update(walletId, (entries) => entries.map((e) => (e.state.kind === "unsure" && e.state.read === "failed" ? { ...e, state: { kind: "unsure", read: "reading" } } : e)));
}

/** Dismissed: only once Mine was read. While an outcome is unknown the notice stays. The
 * attempts stay kept, so a proposal Mine shows later is not credited to a newer one. */
export function dismissKept(walletId: string) {
  update(walletId, (entries) => entries.map((e) => (["found", "done", "twin"].includes(status(e)) ? { ...e, seenAs: status(e) } : e)));
}

/** Why Create, and the fee call, wait for the wallet's kept attempts, or `null` when they do not. */
export function createHeldBack(k: KeptCreate | null): string | null {
  if (k?.kind === "paying") return GovText.keptPaying;
  if (k?.kind !== "unsure") return null;
  switch (k.check.kind) {
    case "reading":
      return GovText.createChecking;
    case "failed":
      return GovText.createReadFailed;
    case "found":
      return GovText.createFound(k.attempt.name);
    case "twin":
      return GovText.createTwin(k.attempt.name);
    case "absent":
      return null;
  }
}

/** Tests: nothing kept. */
export function resetKeptCreate() {
  ledger.clear();
  lastMine.clear();
  views.clear();
  reading.clear();
  listeners.forEach((l) => l());
}
