// The two decisions testnet-fixture.ts must not get wrong offline (H-04 O4a): who holds the
// fixture's lock, and whether a faucet call that did not pay may be asked again. Node built-ins
// only, so vitest checks them.

import { randomBytes } from "node:crypto";
import { linkSync, readFileSync, renameSync, rmSync, statSync, writeFileSync } from "node:fs";

/** The pid in a lock file; null for an empty or unreadable one (held: its writer may be mid-way);
 * "gone" when the file is not there. */
function holderOf(path: string): number | null | "gone" {
  let text: string;
  try {
    text = readFileSync(path, "utf8").trim();
  } catch (e) {
    return (e as NodeJS.ErrnoException).code === "ENOENT" ? "gone" : null;
  }
  return /^[1-9]\d*$/.test(text) ? Number(text) : null;
}

function alive(pid: number): boolean {
  try {
    process.kill(pid, 0);
    return true;
  } catch (e) {
    return (e as NodeJS.ErrnoException).code !== "ESRCH";
  }
}

/** Takes `path` for this process. The pid is written to a private file first and linked into place,
 * so the lock never exists without it. Only a lock naming a pid that no longer runs is taken over,
 * by one process at a time (`<path>.takeover`, holding the taker's pid; a taker that died is
 * cleared); an empty or unreadable lock is held. Throws when held. */
export function takeLock(path: string, pid = process.pid): void {
  const mine = `${path}.${pid}.${randomBytes(4).toString("hex")}`;
  const takeover = `${path}.takeover`;
  writeFileSync(mine, `${pid}\n`, { mode: 0o600, flag: "wx" });
  try {
    for (let i = 0; i < 4; i++) {
      if (tryLink(mine, path)) return;
      const holder = holderOf(path);
      if (holder === "gone") continue;
      if (holder === null) throw new Error(`${path} is held and names no pid: remove it by hand once no fixture run is going`);
      if (alive(holder)) throw new Error(`another fixture run (pid ${holder}) holds ${path}`);
      // Only the takeover's holder removes another's lock, and only the dead holder's own: a new
      // lock can appear only once that one is gone. The takeover file holds its taker's pid.
      if (!tryLink(mine, takeover)) {
        if (!clearStaleTakeover(takeover, `${mine}.stale`)) throw new Error(`another fixture run is taking over ${path}'s stale lock`);
        continue;
      }
      try {
        if (holderOf(path) === holder) rmSync(path);
      } finally {
        // Only this run's own takeover: another may hold the name by now.
        if (statSync(takeover, { throwIfNoEntry: false })?.ino === statSync(mine).ino) rmSync(takeover, { force: true });
      }
    }
    throw new Error(`could not take ${path}`);
  } finally {
    rmSync(mine, { force: true });
  }
}

/** A takeover left by a taker that died (its pid gone, or no readable pid and older than this). */
export const TAKEOVER_STALE_MS = 10 * 60_000;

/** Clears `takeover` if its taker is gone, moving it aside first and putting it back if another
 * replaced it meanwhile; false while a live (or recent, unreadable) taker holds it. */
function clearStaleTakeover(takeover: string, aside: string): boolean {
  const before = statSync(takeover, { throwIfNoEntry: false });
  const taker = holderOf(takeover);
  if (!before || taker === "gone") return true;
  const stale = taker === null ? Date.now() - before.mtimeMs > TAKEOVER_STALE_MS : !alive(taker);
  if (!stale) return false;
  try {
    renameSync(takeover, aside);
  } catch (e) {
    if ((e as NodeJS.ErrnoException).code === "ENOENT") return true;
    throw e;
  }
  const moved = statSync(aside);
  if (moved.ino === before.ino && moved.dev === before.dev) {
    rmSync(aside, { force: true });
    return true;
  }
  try {
    tryLink(aside, takeover);
  } finally {
    rmSync(aside, { force: true });
  }
  return false;
}

function tryLink(from: string, to: string): boolean {
  try {
    linkSync(from, to);
    return true;
  } catch (e) {
    if ((e as NodeJS.ErrnoException).code === "EEXIST") return false;
    throw e;
  }
}

/** Removes `path` if this process holds it. */
export function releaseLock(path: string, pid = process.pid): void {
  if (holderOf(path) === pid) rmSync(path, { force: true });
}

export type FaucetOutcome = { kind: "paid"; txid: string } | { kind: "refused"; why: string } | { kind: "unknown"; why: string };

/** The dash-faucet helper's own lines for a request it never made or the faucet turned down (exit 1);
 * exit 2 is an argument it refused before any request. Of the faucet's HTTP answers only a 429, or a
 * 400 or 403 whose body says why, is a refusal. Anything else may have paid. */
const REFUSALS = [
  /no request was made/,
  /advertises no proof-of-work endpoint/,
  /Could not fetch a proof-of-work challenge/,
  /heavier than the soft tier; refusing/,
  /Proof of work was not accepted/,
  /requires Turnstile/,
  /requires a dash\.org sign-in/,
];

/** The faucet's JSON answer (the helper prints it) names an error. */
function refusalBody(stdout: string | null | undefined): boolean {
  try {
    const b = JSON.parse(stdout ?? "") as { error?: unknown; message?: unknown; detail?: unknown };
    const d = b.detail as { error?: unknown; message?: unknown } | string | undefined;
    return [b.error, b.message, typeof d === "string" ? d : (d?.error ?? d?.message)].some((v) => typeof v === "string" && v.trim() !== "");
  } catch {
    return false;
  }
}

/** What one run of the faucet helper did: paid (exit 0 with a txid), refused (the helper says so),
 * or unknown (a timeout, a 5xx, "Outcome unknown", an unreadable answer): the payout may exist. */
export function faucetOutcome(r: { status: number | null; stdout?: string | null; stderr?: string | null; error?: Error }): FaucetOutcome {
  const stderr = r.stderr ?? "";
  const why = r.error ? r.error.message : stderr.trim().split("\n").slice(-2).join(" ") || `exit ${r.status}`;
  if (r.status === 0) {
    try {
      const txid = (JSON.parse(r.stdout ?? "") as { txid?: unknown }).txid;
      if (typeof txid === "string" && /^[0-9a-f]{64}$/.test(txid)) return { kind: "paid", txid };
    } catch {
      // An exit of 0 with no readable txid: unknown.
    }
    return { kind: "unknown", why: `exit 0 without a txid: ${why}` };
  }
  if (r.error || r.status === null || /Outcome unknown/.test(stderr)) return { kind: "unknown", why };
  if (r.status === 2 || (r.status === 1 && REFUSALS.some((re) => re.test(stderr)))) return { kind: "refused", why };
  const http = r.status === 1 ? /Faucet returned HTTP (\d{3});/.exec(stderr)?.[1] : undefined;
  if (http === "429" || ((http === "400" || http === "403") && refusalBody(r.stdout))) return { kind: "refused", why };
  return { kind: "unknown", why };
}
