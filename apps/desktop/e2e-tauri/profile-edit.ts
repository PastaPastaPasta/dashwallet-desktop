// The one profile edit profile-write.ts makes (BL-162), and its record: as soon as the app confirms
// the edit, <fixture>/BL-162.json says so, before any screenshot or read back can fail, so a rerun
// of the testnet suite never pays for a second edit by mistake. A record that cannot be written
// fails the flow. Node built-ins only, so vitest checks it.

import { renameSync, writeFileSync } from "node:fs";
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
}

/** Writes `ledger` whole or not at all. */
function record(ledger: string, value: unknown) {
  const tmp = `${ledger}.${process.pid}.tmp`;
  writeFileSync(tmp, `${JSON.stringify(value, null, 2)}\n`, { mode: 0o600 });
  renameSync(tmp, ledger);
}

/** The edit under a single-use grant, its record, a screenshot, then a read back. */
export async function writeProfile(invoke: Invoke, screenshot: (name: string) => Promise<void>, e: Edit, step: (s: string) => void, mask: (s: string) => string): Promise<WriteResult> {
  const grant = await invoke<AuthGrant>("authorize", {
    purpose: { kind: "platformOp", maxDuffs: 0, maxCredits: "300000000" },
    walletId: e.walletId,
    credential: { kind: "passphrase", passphrase: e.passphrase },
  });
  if (!grant.ok) throw new Error(`authorize: ${JSON.stringify(grant.err)}`);
  const edit = { displayName: e.label, publicMessage: `${e.label}, written ${e.writeAt}`, avatar: { kind: "keep" } };
  step(`writing the profile (${e.phase})`);
  const t = Date.now();
  const r = await invoke<null>("dashpay_update_profile", { walletId: e.walletId, identity: e.identity, edit, grant: grant.ok.id });
  const ms = Date.now() - t;
  const ok = r.err === undefined;
  const write: WriteResult = { at: e.writeAt, phase: e.phase, ms, ok, error: ok ? null : mask(JSON.stringify(r.err)), label: e.label };
  step(`write ${ok ? "OK" : `FAILED ${write.error}`} in ${ms} ms`);
  if (ok) {
    try {
      record(e.ledger, { state: "done", at: new Date().toISOString(), identity: e.identity, label: e.label });
    } catch (err) {
      throw new Error(`the edit went out but ${e.ledger} could not be written (a rerun would edit again): ${err instanceof Error ? err.message : String(err)}`);
    }
    step(`recorded in ${e.ledger}`);
  }
  await screenshot("2-after-write");
  if (ok) {
    const back = await invoke<DashPayProfile | null>("dashpay_profile", { walletId: e.walletId, identity: e.identity });
    step(`read back: ${JSON.stringify(back.ok?.displayName ?? back.err)}`);
    write.readBack = back.ok?.displayName ?? null;
  }
  return write;
}
