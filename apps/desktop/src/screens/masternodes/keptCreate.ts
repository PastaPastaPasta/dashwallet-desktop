// A proposal's fee payment that outlived its sheet, or whose outcome is not known yet: a lock
// takes the page down at once, and `create_proposal` may already have paid the fee. Calling it
// again with the same draft pays a second fee, so Create (and the fee call itself) waits until
// Mine, read since the unlock, says whether the attempt made its proposal. Only the page draws
// this, and the page is not mounted while locked.

import { useSyncExternalStore } from "react";
import type { MyProposal } from "../../api/generated/MyProposal";
import { lockGeneration, walletGateClosed } from "../../api/lockGate";
import { GovText, inProgress } from "../../governance/model";

/** One press of Pay: the draft's name, and the proposals Mine held as it was pressed. */
export type CreateAttempt = { id: number; name: string; known: readonly string[] };

/** What Mine, read since the unlock, said of an attempt whose call failed. */
export type MineCheck = { kind: "reading" } | { kind: "found"; p: MyProposal } | { kind: "absent" } | { kind: "failed" };

export type KeptCreate =
  | { kind: "paying"; attempt: CreateAttempt }
  | { kind: "created"; p: MyProposal }
  | { kind: "unsure"; attempt: CreateAttempt; check: MineCheck };

const kept = new Map<string, KeptCreate>();
const reading = new Set<string>();
const listeners = new Set<() => void>();
const subscribe = (l: () => void) => {
  listeners.add(l);
  return () => void listeners.delete(l);
};

function put(walletId: string, k: KeptCreate | null) {
  if (k) kept.set(walletId, k);
  else kept.delete(walletId);
  listeners.forEach((l) => l());
}

export const keptCreate = (walletId: string): KeptCreate | null => kept.get(walletId) ?? null;

export const useKeptCreate = (walletId: string): KeptCreate | null => useSyncExternalStore(subscribe, () => keptCreate(walletId));

let attempts = 0;

/** Pay was pressed: until the call settles, Create waits. */
export function startCreate(walletId: string, name: string, mine: readonly MyProposal[]): CreateAttempt {
  const attempt = { id: ++attempts, name, known: mine.map((p) => p.hash) };
  put(walletId, { kind: "paying", attempt });
  return attempt;
}

const current = (walletId: string, attempt: CreateAttempt): boolean => {
  const k = kept.get(walletId);
  return k !== undefined && k.kind !== "created" && k.attempt.id === attempt.id;
};

/** The call settled: `null` when its sheet showed the outcome, or the fee was never paid. Ignored
 * once Mine already answered for the attempt, or another took its place. */
export function settleCreate(walletId: string, attempt: CreateAttempt, outcome: { kind: "created"; p: MyProposal } | { kind: "unsure" } | null) {
  const k = kept.get(walletId);
  if (k?.kind !== "paying" || k.attempt.id !== attempt.id) return;
  put(walletId, outcome?.kind === "unsure" ? { kind: "unsure", attempt, check: { kind: "reading" } } : outcome);
}

/** The attempt's proposal in `mine`: one of its name that Mine did not hold when Pay was pressed. */
export const madeBy = (mine: readonly MyProposal[], attempt: CreateAttempt): MyProposal | undefined =>
  mine.find((p) => p.name === attempt.name && inProgress(p) && !attempt.known.includes(p.hash));

/** Mine as the page read it: an attempt still paying, or one Mine did not show before, ends once
 * it shows the attempt's proposal. */
export function sawMine(walletId: string, mine: readonly MyProposal[]) {
  const k = kept.get(walletId);
  if (!k || k.kind === "created") return;
  const p = madeBy(mine, k.attempt);
  if (!p) return;
  if (k.kind === "paying") put(walletId, { kind: "created", p });
  else if (k.check.kind === "absent" || k.check.kind === "failed") put(walletId, { ...k, check: { kind: "found", p } });
}

/** Reads Mine for an unsure attempt while the window is unlocked. A read the vault locked during
 * began before that lock, so it is read again. */
export async function checkKept(walletId: string, read: () => Promise<MyProposal[]>) {
  const k = kept.get(walletId);
  if (k?.kind !== "unsure" || k.check.kind !== "reading" || reading.has(walletId) || walletGateClosed()) return;
  const generation = lockGeneration();
  reading.add(walletId);
  let check: MineCheck;
  try {
    const p = madeBy(await read(), k.attempt);
    check = p ? { kind: "found", p } : { kind: "absent" };
  } catch {
    check = { kind: "failed" };
  } finally {
    reading.delete(walletId);
  }
  if (!current(walletId, k.attempt)) return;
  const late = walletGateClosed() || lockGeneration() !== generation;
  put(walletId, { kind: "unsure", attempt: k.attempt, check: late ? { kind: "reading" } : check });
}

/** The page showed a made proposal. */
export function keepCreateDone(walletId: string) {
  if (kept.get(walletId)?.kind === "created") put(walletId, null);
}

/** Read again: Mine could not be read. */
export function readKeptAgain(walletId: string) {
  const k = kept.get(walletId);
  if (k?.kind === "unsure" && k.check.kind === "failed") put(walletId, { ...k, check: { kind: "reading" } });
}

/** Dismissed: only once Mine was read. While the outcome is unknown the notice stays. */
export function dismissKept(walletId: string) {
  const k = kept.get(walletId);
  if (k?.kind === "unsure" && (k.check.kind === "found" || k.check.kind === "absent")) put(walletId, null);
}

/** Why Create, and the fee call, wait for the wallet's kept attempt, or `null` when they do not. */
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
    case "absent":
      return null;
  }
}

/** Tests: nothing kept. */
export function resetKeptCreate() {
  kept.clear();
  reading.clear();
  listeners.forEach((l) => l());
}
