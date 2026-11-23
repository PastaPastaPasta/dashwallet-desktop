// The secrets a tauri-driver flow makes itself (H-04 O4a): its app passphrase, registration's new
// wallet. In a suite run (scripts/e2e-tauri-dashpay-testnet.sh sets DW_SECRET_MANIFEST and
// DW_SECRET_RUN_PREFIX) each one goes to the secret store as it is made, and its store name (never
// its value) to the run's manifest, which the closing phrase and key check reads. WebDriver's
// screenshot refuses to capture while a suite flow has registered nothing, a backstop: each flow
// must register a secret before it shows it (a passphrase before the driver starts, E's phrase as it
// is read). Run alone, a flow registers nothing.

import { randomBytes } from "node:crypto";
import { appendFileSync, lstatSync, writeFileSync } from "node:fs";
import { spawnSync } from "node:child_process";

let registered = 0;

function list(manifest: string, entry: object) {
  appendFileSync(manifest, `${JSON.stringify(entry)}\n`, { mode: 0o600 });
  registered++;
}

/** A new random passphrase (not yet registered). */
export const randomPassphrase = () => randomBytes(18).toString("base64url");

/** Stores `value` as `<DW_SECRET_RUN_PREFIX>-<what>` (linked at `link` if given) and lists the name
 * in DW_SECRET_MANIFEST. Throws if the store refuses: the flow must not go on with a secret the
 * closing check cannot know. */
export function registerSecret(what: string, value: string, link?: string): void {
  const manifest = process.env.DW_SECRET_MANIFEST;
  if (!manifest) return;
  const prefix = process.env.DW_SECRET_RUN_PREFIX;
  if (!prefix) throw new Error("DW_SECRET_MANIFEST is set without DW_SECRET_RUN_PREFIX");
  const name = `${prefix}-${what}`;
  const args = ["set", name, "--note", "dwd QA testnet suite run (testnet only)", ...(link ? ["--link", link] : [])];
  const r = spawnSync("secret", args, { input: value, encoding: "utf8" });
  // `secret set` also exits 1 for a sync problem elsewhere in the store: what counts is that the
  // value landed.
  const landed = () => spawnSync("secret", ["get", name], { encoding: "utf8" }).stdout?.trim() === value.trim();
  if (r.status !== 0 && !landed()) throw new Error(`could not put ${what} in the secret store: ${r.error?.message ?? r.stderr?.trim() ?? `exit ${r.status}`}`);
  list(manifest, { name, ...(link ? { link } : {}) });
}

/** A new app passphrase, registered before it is returned. */
export function newPassphrase(what = "passphrase"): string {
  const passphrase = randomPassphrase();
  registerSecret(what, passphrase);
  return passphrase;
}

/** Keeps `value` at `file`, never over an existing one: in a suite run a link into the store
 * (registered), alone a 0600 file. */
export function keepSecretAt(what: string, value: string, file: string): void {
  if (!process.env.DW_SECRET_MANIFEST) return writeFileSync(file, `${value}\n`, { flag: "wx", mode: 0o600 });
  if (lstatSync(file, { throwIfNoEntry: false })) throw new Error(`${file} exists: not storing ${what} over it`);
  registerSecret(what, value, file);
}

/** Lists a secret an earlier run kept at `file` (a link into the store) in this run's manifest. */
export function noteSecretAt(file: string): void {
  const manifest = process.env.DW_SECRET_MANIFEST;
  if (!manifest) return;
  if (!lstatSync(file).isSymbolicLink()) throw new Error(`${file} is a plain file, not a link into the secret store`);
  list(manifest, { link: file });
}

/** Throws when a suite flow is about to capture before registering any secret. */
export function assertCaptureAllowed(): void {
  if (process.env.DW_SECRET_MANIFEST && registered === 0) throw new Error("a screenshot before the flow registered its secrets in DW_SECRET_MANIFEST");
}
