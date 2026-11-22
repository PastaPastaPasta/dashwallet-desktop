// What scripts/e2e-tauri-dashpay-testnet.sh needs from node (H-04 O4a):
//
//   node e2e-tauri/testnet-suite.ts env <fixture>
//     prints `export NAME='value'` lines that point each flow at the fixture testnet-fixture.ts made
//     (birth height, usernames, B's display name, A's contacts); a variable already set is left alone.
//   node e2e-tauri/testnet-suite.ts secrets <fixture>
//     fails unless every phrase and passphrase file of the fixture is a link into the secret store
//     (DW_SECRET_STORE, default ~/.secrets) that resolves, and every one its ledgers say exists is
//     there: each wallet's phrase, the vault passphrase, a finished registration's E files.
//   node e2e-tauri/testnet-suite.ts summary <out> <fixture> [<dir>…]
//     reads <out>/runs.tsv (flow, exit status, seconds, log), writes <out>/summary.json and
//     <out>/summary.md (each flow's result, duration and first error line), then runs the phrase and
//     key check (artifact-scan.ts) over <out> and each <dir>, for the fixture's secrets and those
//     the flows made (<out>/secrets.manifest, flow-secrets.ts). Exits 1 when a flow failed or the
//     check found anything.

import { spawnSync } from "node:child_process";
import { existsSync, lstatSync, readdirSync, readFileSync, realpathSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { basename, dirname, join, relative } from "node:path";
import { scanArtifacts, suspicious } from "./artifact-scan.ts";
import { inside } from "./scratch.ts";

interface Row {
  wallet_id: string;
  identity_id: string;
  username: string;
  display_name?: string;
  birth_height: number;
  contacts: string[];
}

const SAFE = /^[A-Za-z0-9,_. -]+$/;

function envLines(fixtureDir: string): string[] {
  const fixture = JSON.parse(readFileSync(join(fixtureDir, "fixture.json"), "utf8")) as Record<string, Row>;
  const row = (w: string) => {
    const r = fixture[w];
    if (!r?.username || !r.identity_id) throw new Error(`fixture.json has no identity and username for wallet ${w}`);
    return r;
  };
  const birth = Math.min(...Object.values(fixture).map((r) => r.birth_height));
  const vars: Record<string, string> = {
    DW_CONTACTS_EXPECT: row("A").contacts.map((w) => row(w).username).join(","),
    DW_ADD_USERNAME: row("D").username,
    DW_PAY_USERNAME: row("B").username,
  };
  // B's display name, unless the payee was pointed elsewhere.
  const payee = process.env.DW_PAY_USERNAME ?? vars.DW_PAY_USERNAME;
  const b = row("B");
  if (b.display_name && payee === b.username) vars.DW_PAY_DISPLAY_NAME = b.display_name;
  for (const v of ["STATUS", "CONTACTS", "ADD", "BELL", "PAY", "PROFILE", "REG_FUNDER"]) vars[`DW_${v}_BIRTH_HEIGHT`] = String(birth);
  return Object.entries(vars)
    .filter(([k]) => process.env[k] === undefined)
    .map(([k, v]) => {
      if (!SAFE.test(v)) throw new Error(`${k} would be ${JSON.stringify(v)}`);
      return `export ${k}='${v}'`;
    });
}

/** The line that says why a flow failed: its own FAIL line, else the last line of its log. */
function firstError(log: string): string | null {
  const lines = readFileSync(log, "utf8").split("\n").map((l) => l.trim()).filter(Boolean);
  const line = lines.find((l) => /: FAIL: /.test(l)) ?? lines.find((l) => /^(Error|error)[:[ ]/.test(l)) ?? lines.at(-1) ?? null;
  return line && line.length > 400 ? `${line.slice(0, 400)}…` : line;
}

/** Every secret file of the fixture: those its ledgers say exist (each wallet's phrase, the vault
 * passphrase, and a finished registration's E phrase and passphrase) and any other phrase or
 * registration passphrase lying there. */
function secretFiles(fixtureDir: string): { required: string[]; found: string[] } {
  const fixture = JSON.parse(readFileSync(join(fixtureDir, "fixture.json"), "utf8")) as Record<string, Row>;
  const regState = process.env.DW_REG_STATE ?? `${fixtureDir}.reg-state`;
  const required = [...Object.keys(fixture).map((w) => join(fixtureDir, `${w}.mnemonic`)), join(fixtureDir, "passphrase")];
  const reg = join(fixtureDir, "DP1-09.json");
  if (existsSync(reg) && (JSON.parse(readFileSync(reg, "utf8")) as { state?: string }).state === "done") required.push(join(fixtureDir, "E.mnemonic"), join(regState, "passphrase"));
  // Registration's passphrases: DW_REG_STATE and the runs moved aside beside it (<fixture>.reg-state*).
  const parent = dirname(fixtureDir);
  const found = [
    ...readdirSync(fixtureDir).filter((f) => f.endsWith(".mnemonic")).map((f) => join(fixtureDir, f)),
    ...readdirSync(parent, { withFileTypes: true })
      .filter((d) => d.isDirectory() && d.name.startsWith(`${basename(fixtureDir)}.reg-state`))
      .map((d) => join(parent, d.name, "passphrase")),
    join(regState, "passphrase"),
  ].filter((f) => lstatSync(f, { throwIfNoEntry: false }));
  return { required, found: [...new Set([...required, ...found])] };
}

/** Fails unless every secret file of the fixture is a link into the secret store that resolves; a
 * required one that is missing fails too. */
function checkStored(fixtureDir: string): string[] {
  const store = realpathSync(process.env.DW_SECRET_STORE ?? join(homedir(), ".secrets"));
  const { required, found } = secretFiles(fixtureDir);
  const problems = [
    ...required.filter((f) => !lstatSync(f, { throwIfNoEntry: false })).map((f) => `${f} is missing (its ledger says it exists)`),
    ...found.flatMap((f) => {
      const st = lstatSync(f, { throwIfNoEntry: false });
      if (!st) return [];
      if (!st.isSymbolicLink()) return [`${f} is a plain file, not a link into the secret store (${store})`];
      if (!existsSync(f)) return [`${f} is a dangling link: run \`secret sync\` first`];
      return inside(store, realpathSync(f)) ? [] : [`${f} does not point into the secret store (${store})`];
    }),
  ];
  if (problems.length) throw new Error(problems.join("; "));
  return found;
}

/** Each phrase and passphrase the fixture holds, for the check, and what is wrong with how they are
 * kept (checkStored): the summary is written either way, with each problem among its hits. */
function fixtureSecrets(fixtureDir: string): { secrets: string[]; problems: string[] } {
  const problems: string[] = [];
  try {
    checkStored(fixtureDir);
  } catch (e) {
    problems.push(`${join(fixtureDir, "fixture.json")}: ${e instanceof Error ? e.message : String(e)}`);
  }
  let secrets: string[] = [];
  try {
    secrets = secretFiles(fixtureDir).found.filter((f) => existsSync(f)).flatMap((f) => withSpaced(readFileSync(f, "utf8")));
  } catch {
    // checkStored has said why.
  }
  if (secrets.length === 0) problems.push(`${join(fixtureDir, "fixture.json")}: no phrase to check for`);
  return { secrets, problems };
}

const withSpaced = (value: string) => {
  const s = value.trim();
  return s ? [s, s.split(/\s+/).join(" ")] : [];
};

/** The secrets the run's flows made (flow-secrets.ts): read from the store by the names in
 * <out>/secrets.manifest, or through the links it lists. One that cannot be read is a problem. */
function manifestSecrets(out: string): { secrets: string[]; problems: string[] } {
  const manifest = join(out, "secrets.manifest");
  const secrets: string[] = [];
  const problems: string[] = [];
  if (!existsSync(manifest)) return { secrets, problems };
  for (const line of readFileSync(manifest, "utf8").split("\n").filter(Boolean)) {
    let entry: { name?: string; link?: string };
    try {
      entry = JSON.parse(line) as typeof entry;
    } catch {
      problems.push(`${manifest}: a line that is not JSON`);
      continue;
    }
    const r = entry.name ? spawnSync("secret", ["get", entry.name], { encoding: "utf8" }) : null;
    const value = r ? (r.status === 0 ? r.stdout : null) : entry.link && existsSync(entry.link) ? readFileSync(entry.link, "utf8") : null;
    if (value?.trim()) secrets.push(...withSpaced(value));
    else problems.push(`${manifest}: ${entry.name ?? entry.link} cannot be read`);
  }
  return { secrets, problems };
}

function summary(out: string, fixtureDir: string, extra: string[]): number {
  const kept = fixtureSecrets(fixtureDir);
  const made = manifestSecrets(out);
  const secrets = [...kept.secrets, ...made.secrets];
  // A flow masks its own errors; this is the backstop for the one line the summary quotes.
  const clean = (line: string | null) => {
    if (!line) return line;
    const kinds = suspicious(line);
    if (secrets.some((s) => line.includes(s))) kinds.push("known-secret");
    return kinds.length ? `[withheld: ${kinds.join(", ")}]` : line;
  };
  const runs = readFileSync(join(out, "runs.tsv"), "utf8")
    .split("\n")
    .filter(Boolean)
    .map((l) => {
      const [flow, status, seconds, log] = l.split("\t");
      const code = Number(status);
      const result = status === "skip" ? "skip" : code === 0 ? "pass" : "fail";
      return { flow: flow!, result, exit: status === "skip" ? null : code, seconds: Number(seconds), error: result === "fail" ? clean(firstError(log!)) : null, log: relative(out, log!) };
    });
  const scan = scanArtifacts([out, ...extra.filter(existsSync)], secrets);
  scan.hits.push(...kept.problems, ...made.problems);
  const at = new Date().toISOString();
  const json = { at, flows: runs, scan: { files: scan.files, ocr: scan.ocr, clean: scan.hits.length === 0, hits: scan.hits.map((h) => relative(out, h)) } };
  writeFileSync(join(out, "summary.json"), `${JSON.stringify(json, null, 2)}\n`);
  const md = [
    `# DashPay GL flows on testnet (${at})`,
    "",
    "| Flow | Result | Duration | First error |",
    "| --- | --- | --- | --- |",
    ...runs.map((r) => `| ${r.flow} | ${r.result.toUpperCase()} | ${Math.floor(r.seconds / 60)}m${String(r.seconds % 60).padStart(2, "0")}s | ${(r.error ?? "").replace(/\|/g, "\\|")} |`),
    "",
    `Phrase and key check (known secrets, BIP39 runs of 12+, xprv/tprv${scan.ocr ? ", screenshots through tesseract" : "; no tesseract, so every screenshot counts as a hit"}): ${scan.files} files, ${scan.hits.length ? `${scan.hits.length} HITS:\n\n${scan.hits.map((h) => `- ${relative(out, h)}`).join("\n")}` : "clean"}.`,
    "",
  ].join("\n");
  writeFileSync(join(out, "summary.md"), md);
  console.log(md);
  return runs.some((r) => r.result === "fail") || scan.hits.length ? 1 : 0;
}

const [cmd, ...args] = process.argv.slice(2);
try {
  if (cmd === "env" && args[0]) console.log(envLines(args[0]).join("\n"));
  else if (cmd === "secrets" && args[0]) checkStored(args[0]);
  else if (cmd === "summary" && args[0] && args[1]) process.exitCode = summary(args[0], args[1], args.slice(2));
  else throw new Error("usage: testnet-suite.ts env <fixture> | summary <out> <fixture> [<dir>…]");
} catch (e) {
  console.error(`testnet-suite: ${e instanceof Error ? e.message : String(e)}`);
  process.exitCode = 2;
}
