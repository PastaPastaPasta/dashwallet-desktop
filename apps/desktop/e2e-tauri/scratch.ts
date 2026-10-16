// Where a tauri-driver run may write and delete (BL-162): one scratch root holds every runtime file
// of a run, caller paths are canonicalized and refused outside it, a leak sweep reads anywhere but
// deletes only inside directories the run created, and directory walks never follow symlinks.
// Node built-ins only, so vitest checks it without the driver's environment.

import { lstatSync, mkdirSync, readdirSync, readFileSync, realpathSync, rmSync } from "node:fs";
import { basename, dirname, isAbsolute, join, relative, resolve, sep } from "node:path";

/** Where a write to `p` lands: `p` absolute, with every existing ancestor's symlinks resolved. A
 * dangling symlink on the way throws. */
export function canonical(p: string): string {
  const abs = resolve(p);
  if (lstatSync(abs, { throwIfNoEntry: false })) return realpathSync(abs);
  const parent = dirname(abs);
  return parent === abs ? abs : join(canonical(parent), basename(abs));
}

/** `p` lies strictly below `dir` (both canonical). */
export function inside(dir: string, p: string): boolean {
  const r = relative(dir, p);
  return r !== "" && r !== ".." && !r.startsWith(`..${sep}`) && !isAbsolute(r);
}

/** The scratch root, canonicalized, refused inside `checkout` (before anything is created), and
 * created 0700 if new. */
export function scratchRoot(wanted: string, checkout: string): string {
  const root = canonical(wanted);
  const repo = canonical(checkout);
  if (root === repo || inside(repo, root)) throw new Error(`${wanted}: the scratch root may not be inside the checkout ${repo}`);
  mkdirSync(root, { recursive: true, mode: 0o700 });
  return root;
}

/** A new directory at `wanted`, which must lie below `root` and not exist yet: the run owns it. */
export function newDirIn(root: string, wanted: string): string {
  const p = canonical(wanted);
  if (!inside(root, p)) throw new Error(`${wanted}: outside the scratch root ${root}`);
  if (lstatSync(p, { throwIfNoEntry: false })) throw new Error(`${wanted}: exists; a run writes only to a directory it creates`);
  mkdirSync(p, { recursive: true, mode: 0o700 });
  return p;
}

/** Every regular file under `dir` (the data root, OUT) that holds one of `secrets` as plain text, as
 * `<path> (<index of the secret>)`. Symlinks are skipped, so the walk stays below `dir`. */
export function leaks(dir: string, secrets: string[]): string[] {
  const found: string[] = [];
  const walk = (d: string) => {
    for (const name of readdirSync(d)) {
      const p = join(d, name);
      const st = lstatSync(p, { throwIfNoEntry: false });
      if (!st) continue;
      if (st.isDirectory()) walk(p);
      else if (st.isFile()) {
        const text = readFileSync(p).toString("latin1");
        for (const s of secrets) if (text.includes(Buffer.from(s, "utf8").toString("latin1"))) found.push(`${p} (${secrets.indexOf(s)})`);
      }
    }
  };
  walk(canonical(dir));
  return found;
}

/** Deletes each of `found` (as `leaks` reports them) that is a regular file inside one of `owned`;
 * returns the others, left in place. */
export function removeOwned(found: string[], owned: string[]): string[] {
  const kept: string[] = [];
  for (const f of found) {
    const p = f.replace(/ \(\d+\)$/, "");
    if (owned.some((o) => inside(o, p)) && lstatSync(p, { throwIfNoEntry: false })?.isFile()) rmSync(p, { force: true });
    else kept.push(f);
  }
  return kept;
}
