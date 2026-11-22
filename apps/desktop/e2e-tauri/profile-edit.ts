// The one profile edit profile-write.ts makes (BL-162), and its record <fixture>/BL-162.json: pending
// before the edit is submitted (no record, no submit), done only once the profile reads back what was
// submitted. A rerun that finds pending reads the profile instead of submitting, so a timeout after
// Platform took the edit never pays for a second one. Node built-ins only, so vitest checks it.

import { existsSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import type { AuthGrant } from "../src/api/generated/AuthGrant";
import type { DashPayProfile } from "../src/api/generated/DashPayProfile";

export interface Answer<T> {
  ok?: T;
  err?: unknown;
}

type Invoke = <T>(cmd: string, args?: Record<string, unknown>) => Promise<Answer<T>>;

export interface WriteResult {
  at: string;
  phase: string;
  ms: number;
  ok: boolean;
  error: string | null;
  label: string;
  readBack?: string | null;
  /** An earlier run's pending edit, settled by reading it back. */
  resumed?: boolean;
}

interface Edit {
  walletId: string;
  identity: string;
  passphrase: string;
  phase: string;
  label: string;
  writeAt: string;
  /** The ledger the suite reads (<fixture>/BL-162.json). */
  ledger: string;
  /** Submit even over a pending record (DW_PROFILE_WRITE_AGAIN=1). */
  again: boolean;
}

interface Entry {
  state: "pending" | "done";
  at: string;
  identity: string;
  label: string;
}

/** Writes `ledger` whole or not at all. */
function record(ledger: string, entry: Entry) {
  const tmp = `${ledger}.${process.pid}.tmp`;
  writeFileSync(tmp, `${JSON.stringify(entry, null, 2)}\n`, { mode: 0o600 });
  renameSync(tmp, ledger);
}

function earlier(ledger: string): Entry | null {
  if (!existsSync(ledger)) return null;
  return JSON.parse(readFileSync(ledger, "utf8")) as Entry;
}

/** The edit under a single-use grant, recorded `pending` before it is submitted and `done` only once
 * the profile reads back the label; then a screenshot. A `pending` record from an earlier run (the
 * edit's outcome unknown) is settled by reading the profile, never by submitting again, unless
 * `again` (DW_PROFILE_WRITE_AGAIN=1). */
export async function writeProfile(invoke: Invoke, screenshot: (name: string) => Promise<void>, e: Edit, step: (s: string) => void, mask: (s: string) => string): Promise<WriteResult> {
  const readBack = async () => {
    const back = await invoke<DashPayProfile | null>("dashpay_profile", { walletId: e.walletId, identity: e.identity });
    step(`read back: ${JSON.stringify(back.ok?.displayName ?? back.err)}`);
    return back.err === undefined ? (back.ok?.displayName ?? null) : undefined;
  };
  const shown = (v: string | null | undefined) => (v === undefined ? "an error" : JSON.stringify(v));
  const mark = (state: Entry["state"], label: string) => record(e.ledger, { state, at: new Date().toISOString(), identity: e.identity, label });

  const before = earlier(e.ledger);
  if (before && !e.again) {
    const held = `${e.ledger} holds an edit to ${JSON.stringify(before.label)}`;
    if (before.state === "done") throw new Error(`${held}, done: not submitting another (DW_PROFILE_WRITE_AGAIN=1 submits a new edit)`);
    if (before.state !== "pending") throw new Error(`${held} in state ${JSON.stringify(before.state)}: not submitting`);
    if (before.identity !== e.identity) throw new Error(`${held} for identity ${before.identity}, not ${e.identity}, whose outcome is unknown: not submitting; check that identity's profile by hand`);
    const back = await readBack();
    if (back === undefined) throw new Error(`${held} whose outcome is unknown, and the profile could not be read: not submitting; rerun later`);
    if (back !== before.label)
      throw new Error(`${held} whose outcome is unknown, and the profile reads ${shown(back)}: not submitting; wait and rerun, as Platform may not show it yet (if it never does, DW_PROFILE_WRITE_AGAIN=1 submits a new edit)`);
    mark("done", before.label);
    step(`the earlier edit to ${JSON.stringify(before.label)} went through; recorded done, nothing submitted`);
    await screenshot("2-after-write");
    return { at: e.writeAt, phase: e.phase, ms: 0, ok: true, error: null, label: before.label, readBack: back, resumed: true };
  }

  const grant = await invoke<AuthGrant>("authorize", {
    purpose: { kind: "platformOp", maxDuffs: 0, maxCredits: "300000000" },
    walletId: e.walletId,
    credential: { kind: "passphrase", passphrase: e.passphrase },
  });
  if (!grant.ok) throw new Error(`authorize: ${JSON.stringify(grant.err)}`);
  try {
    mark("pending", e.label);
  } catch (err) {
    throw new Error(`${e.ledger} could not be written; nothing submitted: ${err instanceof Error ? err.message : String(err)}`);
  }
  const edit = { displayName: e.label, publicMessage: `${e.label}, written ${e.writeAt}`, avatar: { kind: "keep" } };
  step(`writing the profile (${e.phase}); ${e.ledger} says pending`);
  const t = Date.now();
  const r = await invoke<null>("dashpay_update_profile", { walletId: e.walletId, identity: e.identity, edit, grant: grant.ok.id });
  const ms = Date.now() - t;
  if (r.err !== undefined) throw new Error(`the edit failed in ${ms} ms: ${mask(JSON.stringify(r.err))}; ${e.ledger} stays pending`);
  step(`write OK in ${ms} ms`);
  const back = await readBack();
  if (back !== e.label) throw new Error(`the edit reported success but the profile reads ${shown(back)}, not ${JSON.stringify(e.label)}; ${e.ledger} stays pending`);
  try {
    mark("done", e.label);
  } catch (err) {
    throw new Error(`the edit went through but ${e.ledger} could not be marked done (it says pending; a rerun reads the profile): ${err instanceof Error ? err.message : String(err)}`);
  }
  step(`recorded done in ${e.ledger}`);
  await screenshot("2-after-write");
  return { at: e.writeAt, phase: e.phase, ms, ok: true, error: null, label: e.label, readBack: back };
}
