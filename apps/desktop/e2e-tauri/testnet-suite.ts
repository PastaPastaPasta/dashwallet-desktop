// What scripts/e2e-tauri-dashpay-testnet.sh needs from node (H-04 O4a):
//
//   node e2e-tauri/testnet-suite.ts env <fixture>
//     prints `export NAME=value` lines that point each flow at the fixture testnet-fixture.ts made
//     (birth height, usernames, A's contacts); a variable already set is left alone.
//   node e2e-tauri/testnet-suite.ts secrets <fixture>
//     fails when a phrase or passphrase the closing check needs cannot be read (a dangling link).
//   node e2e-tauri/testnet-suite.ts summary <out> <fixture> [<dir>…]
//     reads <out>/runs.tsv (flow, exit status, seconds, log), writes <out>/summary.json and
//     <out>/summary.md (each flow's result, duration and first error line), then runs the phrase and
//     key check (artifact-scan.ts) over <out> and each <dir>. Exits 1 when a flow failed or the
//     check found anything.

import { existsSync, lstatSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { basename, dirname, join, relative } from "node:path";
import { scanArtifacts, suspicious } from "./artifact-scan.ts";

interface Row {
  wallet_id: string;
  identity_id: string;
  username: string;
  birth_height: number;
  contacts: string[];
}

const SAFE = /^[A-Za-z0-9,_.-]+$/;

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
  for (const v of ["STATUS", "CONTACTS", "ADD", "BELL", "PAY", "PROFILE", "REG_FUNDER"]) vars[`DW_${v}_BIRTH_HEIGHT`] = String(birth);
  return Object.entries(vars)
    .filter(([k]) => process.env[k] === undefined)
    .map(([k, v]) => {
      if (!SAFE.test(v)) throw new Error(`${k} would be ${JSON.stringify(v)}`);
      return `export ${k}=${v}`;
    });
}

/** The line that says why a flow failed: its own FAIL line, else the last line of its log. */
function firstError(log: string): string | null {
  const lines = readFileSync(log, "utf8").split("\n").map((l) => l.trim()).filter(Boolean);
  const line = lines.find((l) => /: FAIL: /.test(l)) ?? lines.find((l) => /^(Error|error)[:[ ]/.test(l)) ?? lines.at(-1) ?? null;
  return line && line.length > 400 ? `${line.slice(0, 400)}…` : line;
}

/** Each phrase and passphrase the fixture holds, for the plain-text check. A link the store has not
 * filled yet (`secret sync`) is an error: the check would run without that secret. */
function fixtureSecrets(fixtureDir: string): string[] {
  const out: string[] = [];
  // Registration's passphrases: DW_REG_STATE and the runs moved aside beside it (<fixture>.reg-state*).
  const parent = dirname(fixtureDir);
  const regPasses = [
    join(process.env.DW_REG_STATE ?? `${fixtureDir}.reg-state`, "passphrase"),
    ...readdirSync(parent, { withFileTypes: true })
      .filter((d) => d.isDirectory() && d.name.startsWith(`${basename(fixtureDir)}.reg-state`))
      .map((d) => join(parent, d.name, "passphrase")),
  ];
  const files = [...readdirSync(fixtureDir).filter((f) => f.endsWith(".mnemonic") || f === "passphrase").map((f) => join(fixtureDir, f)), ...new Set(regPasses)];
  for (const f of files) {
    if (regPasses.includes(f) && !lstatSync(f, { throwIfNoEntry: false })) continue;
    if (!existsSync(f)) throw new Error(`${f} is a dangling link: run \`secret sync\` first`);
    const s = readFileSync(f, "utf8").trim();
    if (s) out.push(s, s.split(/\s+/).join(" "));
  }
  if (out.length === 0) throw new Error(`${fixtureDir} holds no phrase to check for`);
  return out;
}

function summary(out: string, fixtureDir: string, extra: string[]): number {
  const secrets = fixtureSecrets(fixtureDir);
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
    `Phrase and key check (known secrets, BIP39 runs of 12+, xprv/tprv${scan.ocr ? ", screenshots through tesseract" : "; no tesseract, screenshots not read"}): ${scan.files} files, ${scan.hits.length ? `${scan.hits.length} HITS:\n\n${scan.hits.map((h) => `- ${relative(out, h)}`).join("\n")}` : "clean"}.`,
    "",
  ].join("\n");
  writeFileSync(join(out, "summary.md"), md);
  console.log(md);
  return runs.some((r) => r.result === "fail") || scan.hits.length ? 1 : 0;
}

const [cmd, ...args] = process.argv.slice(2);
try {
  if (cmd === "env" && args[0]) console.log(envLines(args[0]).join("\n"));
  else if (cmd === "secrets" && args[0]) fixtureSecrets(args[0]);
  else if (cmd === "summary" && args[0] && args[1]) process.exitCode = summary(args[0], args[1], args.slice(2));
  else throw new Error("usage: testnet-suite.ts env <fixture> | summary <out> <fixture> [<dir>…]");
} catch (e) {
  console.error(`testnet-suite: ${e instanceof Error ? e.message : String(e)}`);
  process.exitCode = 2;
}
