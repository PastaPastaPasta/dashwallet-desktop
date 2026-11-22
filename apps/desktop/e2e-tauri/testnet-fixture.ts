// Builds the testnet DashPay fixture the seven GL flows read (H-04 O4a): four new wallets A, B, C
// and D, each with an identity and a non-contested username, and the contact graph DP2-09 D20
// describes: A:{B,C}, B:{A}, C:{A,D}, D:{C}, so B–C, B–D and A–D are not contacts (bell-accept's
// C→B, add-contact's A→D). Funding comes from the dash-faucet skill's helper, once, to A; A pays
// B, C and D. Then one `dwcli dashpay --spv session` registers the identities and sends and
// accepts the contact requests.
//
//   DWCLI=… DW_TESTNET_FIXTURE=<dir> node e2e-tauri/testnet-fixture.ts
//
// <dir> must lie under $HOME (the secret store links only there). It ends up holding:
//   <W>.mnemonic         a symlink into the secret store (`secret set <prefix>-<W>-mnemonic`)
//   passphrase           the vault passphrase of state/, also in the store (`<prefix>-passphrase`)
//   state/               one dwcli data root with all four wallets (kept: a rerun resumes its SPV)
//   fixture-state.json   every step, written before the step acts (no secrets)
//   fixture.json         what the flows read: per wallet its wallet id, identity, username,
//                        birth height, contacts and credits
//
// Idempotent and resumable: each write is recorded as intended before it is made, and a rerun
// reads the network before writing again. A phrase is made once (a store entry is never
// overwritten), the faucet is asked once (a request whose outcome is unknown stops the run for a
// look), A pays B, C and D once, a registration starts only when the wallet has no identity and no
// registration row, and a contact request goes out only when the pair has no relation. Phrases go
// to dwcli on stdin and to `secret` on stdin, never argv or stdout.
//
//   DW_FIXTURE_SECRET_PREFIX   store names' prefix (default dwd-qa-testnet-<basename of dir>)
//   DW_FIXTURE_FAUCET_TDASH    what the faucet pays A (default 3)
//   DW_FIXTURE_SHARE_DUFFS     what A pays each of B, C and D (default 50000000)
//   DW_FAUCET_HELPER           the faucet helper (default the dash-faucet skill's)
//   DW_INSIGHT                 testnet Insight API (default insight.testnet.networks.dash.org)

import { spawn, spawnSync } from "node:child_process";
import { randomBytes, randomInt } from "node:crypto";
import { createInterface } from "node:readline";
import { existsSync, lstatSync, mkdirSync, mkdtempSync, openSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { basename, join, resolve } from "node:path";
import { leaks } from "./scratch.ts";

const env = (name: string, fallback?: string): string => {
  const v = process.env[name] ?? fallback;
  if (!v) throw new Error(`set ${name}`);
  return v;
};

const DWCLI = resolve(env("DWCLI"));
const FIXTURE = resolve(env("DW_TESTNET_FIXTURE"));
const PREFIX = env("DW_FIXTURE_SECRET_PREFIX", `dwd-qa-testnet-${basename(FIXTURE)}`);
const FAUCET_TDASH = env("DW_FIXTURE_FAUCET_TDASH", "3");
const SHARE_DUFFS = Number(env("DW_FIXTURE_SHARE_DUFFS", "50000000"));
const FAUCET_HELPER = env("DW_FAUCET_HELPER", join(homedir(), ".claude/skills/dash-faucet/scripts/request_core_faucet.py"));
const INSIGHT = env("DW_INSIGHT", "https://insight.testnet.networks.dash.org/insight-api");
const SYNC_TIMEOUT_S = Number(env("DW_FIXTURE_SYNC_TIMEOUT_S", "3600"));
const REGISTRATION_TIMEOUT_MS = Number(env("DW_FIXTURE_REGISTRATION_TIMEOUT_MS", "2400000"));
const CONTACT_TIMEOUT_MS = Number(env("DW_FIXTURE_CONTACT_TIMEOUT_MS", "900000"));

const WALLETS = ["A", "B", "C", "D"] as const;
type W = (typeof WALLETS)[number];
/** Who sends, who accepts (DP2-09 D20's graph). */
const EDGES: [W, W][] = [
  ["A", "B"],
  ["A", "C"],
  ["C", "D"],
];
/** Pairs the flows need unrelated: bell-accept C→B, add-contact A→D; B–D for a later pair. */
const STRANGERS: [W, W][] = [
  ["B", "C"],
  ["A", "D"],
  ["B", "D"],
];

const STATE_FILE = join(FIXTURE, "fixture-state.json");
const ROOT = join(FIXTURE, "state");
const PASS_FILE = join(FIXTURE, "passphrase");
const LOGS = join(FIXTURE, "logs");
const NETWORK = ["--network", "testnet"];

interface Registration {
  state: "starting" | "started" | "done";
  label: string;
  draft?: string;
  identity?: string;
  quote?: { lockDuffs: number; totalDuffs: number; remainingCredits: number };
}

interface State {
  vault?: boolean;
  birthHeight?: number;
  wallets: Partial<Record<W, { walletId: string }>>;
  faucet?: { state: "requesting" | "paid"; address: string; tdash: string; txid?: string; at: string };
  shares?: { state: "sending" | "sent"; to: Record<string, string>; duffs: number; txid?: string; at: string };
  registrations: Partial<Record<W, Registration>>;
  contacts: Record<string, { state: "requesting" | "sent" | "accepting" | "contacts"; at: string }>;
  steps: string[];
}

let state: State = { wallets: {}, registrations: {}, contacts: {}, steps: [] };
const save = () => writeFileSync(STATE_FILE, JSON.stringify(state, null, 2), { mode: 0o600 });
function step(s: string) {
  console.log(`fixture: ${s}`);
  state.steps.push(`${new Date().toISOString()} ${s}`);
  save();
}

/** What no output may hold: each phrase and the passphrase. */
const secrets: string[] = [];
function mask(text: string): string {
  const words = [...new Set(secrets.flatMap((s) => s.split(/\s+/)))].filter(Boolean).sort((a, b) => b.length - a.length);
  if (!words.length) return text;
  const alternation = words.map((w) => w.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")).join("|");
  return text.replace(new RegExp(`(?<![\\w-])(?:${alternation})(?![\\w-])`, "g"), "***");
}
const addPhrase = (p: string) => secrets.push(p, p.split(/\s+/).join(" "));

function dwcli(args: string[], input?: string, root = ROOT, timeoutS = 900): string {
  const r = spawnSync(DWCLI, ["--datadir", root, ...NETWORK, "--passphrase-file", PASS_FILE, ...args], { encoding: "utf8", input, timeout: timeoutS * 1000 });
  if (r.status === 0) return r.stdout;
  throw new Error(`dwcli ${args.slice(0, 2).join(" ")} failed (${r.status ?? r.signal}): ${mask(`${r.stderr}${r.stdout}`).slice(-800)}`);
}

function secret(args: string[], input?: string): string {
  const r = spawnSync("secret", args, { encoding: "utf8", input });
  if (r.status !== 0) throw new Error(`secret ${args[0]} ${args[1] ?? ""} failed (${r.status}): ${mask(r.stderr).slice(-400)}`);
  return r.stdout;
}

/** Names in the store (not values). */
const stored = () => new Set(secret(["ls"]).split("\n").map((l) => l.split(/\s+/)[0]));

async function json<T>(url: string): Promise<T> {
  const res = await fetch(url, { headers: { accept: "application/json" } });
  if (!res.ok) throw new Error(`${url}: HTTP ${res.status}`);
  return (await res.json()) as T;
}

const tip = async () => (await json<{ info: { blocks: number } }>(`${INSIGHT}/status?q=getInfo`)).info.blocks;
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

// ------------------------------------------------------------------ secrets and wallets

/** The vault passphrase: made once, kept in the store, linked at PASS_FILE. */
function passphrase(): string {
  const name = `${PREFIX}-passphrase`;
  if (!existsSync(PASS_FILE)) {
    if (stored().has(name)) throw new Error(`${name} is in the store but ${PASS_FILE} is missing: run \`secret sync\``);
    secret(["set", name, "--link", PASS_FILE, "--note", "dwd QA testnet fixture vault passphrase"], `${randomBytes(18).toString("base64url")}\n`);
  }
  return readFileSync(PASS_FILE, "utf8").trim();
}

/** W's phrase: made once by `dwcli create` on a scratch root (its stdout read, never shown), kept in
 * the store, linked at <W>.mnemonic. */
function phrase(w: W): string {
  const file = join(FIXTURE, `${w}.mnemonic`);
  const name = `${PREFIX}-${w}-mnemonic`;
  if (!existsSync(file)) {
    if (stored().has(name)) throw new Error(`${name} is in the store but ${file} is missing: run \`secret sync\``);
    const scratch = mkdtempSync(join(FIXTURE, ".gen-"));
    try {
      dwcli(["init-vault"], undefined, scratch);
      const out = dwcli(["create"], undefined, scratch);
      const words = /^mnemonic (.+)$/m.exec(out)?.[1]?.trim();
      if (!words || words.split(/\s+/).length !== 12) throw new Error("dwcli create printed no 12-word phrase");
      addPhrase(words);
      secret(["set", name, "--link", file, "--note", `dwd QA testnet fixture wallet ${w} (testnet only)`], `${words}\n`);
    } finally {
      rmSync(scratch, { recursive: true, force: true });
    }
  }
  const p = readFileSync(file, "utf8").trim();
  addPhrase(p);
  return p;
}

const listed = () =>
  dwcli(["list"])
    .split("\n")
    .map((l) => /^(\S+) name="([^"]*)"(.*)$/.exec(l))
    .filter((m): m is RegExpExecArray => !!m)
    .map((m) => ({ walletId: m[1]!, name: m[2]!, rest: m[3]! }));

/** The four wallets in state/, A first (the oldest wallet is `dwcli receive`'s and `sync`'s). */
async function wallets() {
  mkdirSync(ROOT, { recursive: true, mode: 0o700 });
  if (!state.vault) {
    try {
      dwcli(["init-vault"]);
    } catch (e) {
      // A run that stopped right after it made the vault.
      if (!/exist/i.test(String(e))) throw e;
    }
    state.vault = true;
  }
  state.birthHeight ??= (await tip()) - 20;
  save();
  for (const w of WALLETS) {
    const p = phrase(w);
    if (state.wallets[w]) continue;
    let found = listed().find((l) => l.name === `Fixture ${w}`);
    if (!found) {
      dwcli(["import", "--birth-height", String(state.birthHeight), "--name", `Fixture ${w}`], `${p}\n`);
      found = listed().find((l) => l.name === `Fixture ${w}`);
    }
    if (!found) throw new Error(`dwcli list shows no Fixture ${w} after the import`);
    state.wallets[w] = { walletId: found.walletId };
    step(`wallet ${w} ${found.walletId} (birth ${state.birthHeight}); phrase in the store as ${PREFIX}-${w}-mnemonic`);
  }
}

const idOf = (w: W) => state.wallets[w]!.walletId;

// ------------------------------------------------------------------ funding

interface InsightTx {
  txid: string;
  vout: { value: string; scriptPubKey: { addresses?: string[] } }[];
}

/** Payments to `address` Insight knows of. */
async function paidTo(address: string): Promise<InsightTx[]> {
  const r = await json<{ txs: InsightTx[] }>(`${INSIGHT}/txs?address=${address}`);
  return r.txs.filter((t) => t.vout.some((o) => o.scriptPubKey.addresses?.includes(address)));
}

/** A fresh receive address of W. */
function addressOf(w: W): string {
  const a = /^address (y[1-9A-HJ-NP-Za-km-z]{33})$/m.exec(dwcli(["address", idOf(w)]))?.[1];
  if (!a) throw new Error(`dwcli address printed no testnet address for ${w}`);
  return a;
}

/** One faucet payout to A, recorded as requesting before the helper runs. */
async function faucet() {
  if (state.faucet?.state === "paid") return;
  if (state.faucet?.state === "requesting") {
    const seen = await paidTo(state.faucet.address);
    if (seen.length === 0)
      throw new Error(`fixture-state.json says the faucet was asked to pay ${state.faucet.address} at ${state.faucet.at} and Insight shows nothing: look (the faucet log, Insight) before running again`);
    state.faucet = { ...state.faucet, state: "paid", txid: seen[0]!.txid };
    step(`the faucet's earlier payout to ${state.faucet.address} arrived: ${seen[0]!.txid}`);
    return;
  }
  const address = addressOf("A");
  state.faucet = { state: "requesting", address, tdash: FAUCET_TDASH, at: new Date().toISOString() };
  step(`asking the faucet for ${FAUCET_TDASH} tDASH to A's ${address}`);
  const r = spawnSync("python3", ["-I", FAUCET_HELPER, address, "--amount", FAUCET_TDASH], { encoding: "utf8", timeout: 300_000 });
  writeFileSync(join(LOGS, `faucet-${Date.now()}.log`), `${r.stdout}\n${r.stderr}`, { mode: 0o600 });
  const txid = (() => {
    try {
      return (JSON.parse(r.stdout) as { txid?: string }).txid;
    } catch {
      return undefined;
    }
  })();
  if (r.status !== 0 || !txid) throw new Error(`the faucet did not pay (${r.status}): ${r.stderr.trim().split("\n").slice(-2).join(" ")}`);
  state.faucet = { ...state.faucet, state: "paid", txid };
  step(`the faucet paid A ${FAUCET_TDASH} tDASH: ${txid}`);
}

/** A pays B, C and D their share in one transaction, recorded as sending first. */
async function shares() {
  if (state.shares?.state === "sent") return;
  if (state.shares?.state === "sending") {
    const seen = await Promise.all(Object.values(state.shares.to).map(paidTo));
    if (seen.every((s) => s.length > 0)) {
      state.shares = { ...state.shares, state: "sent", txid: seen[0]![0]!.txid };
      step(`A's earlier payment to B, C and D arrived: ${state.shares.txid}`);
      return;
    }
    throw new Error(`fixture-state.json says A was paying B, C and D (${state.shares.at}) and Insight does not show it: look before running again`);
  }
  dwcli(["sync", "--txid", state.faucet!.txid!, "--confirmations", "1", "--timeout-secs", String(SYNC_TIMEOUT_S)], undefined, ROOT, SYNC_TIMEOUT_S + 120);
  step("A has the faucet's payout, confirmed");
  const to: Record<string, string> = {};
  for (const w of ["B", "C", "D"] as const) to[w] = addressOf(w);
  state.shares = { state: "sending", to, duffs: SHARE_DUFFS, at: new Date().toISOString() };
  step(`A pays ${SHARE_DUFFS} duffs each to B ${to.B}, C ${to.C}, D ${to.D}`);
  const h = await tip();
  const out = dwcli(["send", idOf("A"), ...Object.values(to).flatMap((a) => ["--to", `${a}:${SHARE_DUFFS}`]), "--sync-height", String(h - 2)]);
  const txid = /^broadcast (\S+)$/m.exec(out)?.[1];
  if (!txid) throw new Error(`dwcli send printed no broadcast txid: ${mask(out).slice(-300)}`);
  state.shares = { ...state.shares, state: "sent", txid };
  step(`A paid B, C and D: ${txid}`);
}

// ------------------------------------------------------------------ the DashPay session

type Answer = { ok: boolean; result?: any; error?: { code: string; message: string } };

/** One `dwcli dashpay --spv session` for the whole DashPay phase: SPV and DashPay's bring-up run
 * once for the four wallets. */
class Session {
  #next = 1;
  #waiting = new Map<number, (a: Answer) => void>();
  #child = spawn(DWCLI, ["--datadir", ROOT, ...NETWORK, "--passphrase-file", PASS_FILE, "--verbose-events", "dashpay", "--spv", "session"], {
    stdio: ["pipe", "pipe", openSync(join(LOGS, `session-${Date.now()}.log`), "w", 0o600)],
  });
  #closed: Promise<number | null>;

  constructor() {
    createInterface({ input: this.#child.stdout! }).on("line", (line) => {
      let a: Answer & { id?: number };
      try {
        a = JSON.parse(line);
      } catch {
        return;
      }
      if (typeof a.id === "number") {
        this.#waiting.get(a.id)?.(a);
        this.#waiting.delete(a.id);
      }
    });
    this.#closed = new Promise((r) => this.#child.on("exit", (code) => r(code)));
    void this.#closed.then((code) => {
      for (const [, done] of this.#waiting) done({ ok: false, error: { code: "session_exit", message: `dwcli session exited (${code})` } });
      this.#waiting.clear();
    });
  }

  ask(...args: string[]): Promise<Answer> {
    const id = this.#next++;
    const what = args.filter((a) => !/^[0-9a-f]{64}$/.test(a)).slice(0, 3).join(" ");
    const started = Date.now();
    const slow = setInterval(() => console.log(`fixture: dwcli ${what} still running (${Math.round((Date.now() - started) / 1000)} s)`), 120_000);
    return new Promise<Answer>((r) => {
      this.#waiting.set(id, r);
      this.#child.stdin!.write(`${JSON.stringify({ args, id })}\n`);
    }).finally(() => clearInterval(slow));
  }

  async run<T = any>(...args: string[]): Promise<T> {
    const a = await this.ask(...args);
    if (!a.ok) throw new Error(`${args.slice(0, 2).join(" ")}: ${a.error?.code}: ${mask(a.error?.message ?? "")}`);
    return a.result as T;
  }

  async close() {
    this.#child.stdin!.end();
    await Promise.race([this.#closed, sleep(120_000)]);
    this.#child.kill("SIGTERM");
  }
}

interface Row {
  draft: string;
  phase: string;
  label: string;
  identity: string | null;
  txid: string | null;
  waiting: string | null;
  running: boolean;
  failure: { code: string; phase: string } | null;
  name_failure: string | null;
}

/** A non-contested label (a digit 2–9, DIP-23). */
const newLabel = (w: W) => `dwo4a${w.toLowerCase()}${Array.from({ length: 4 }, () => randomInt(2, 10)).join("")}`;

async function poll<T>(ms: number, what: string, every: number, probe: () => Promise<T | undefined>): Promise<T> {
  const until = Date.now() + ms;
  const started = Date.now();
  let notedAt = started;
  for (;;) {
    if (Date.now() - notedAt > 120_000) {
      console.log(`fixture: still waiting for ${what} (${Math.round((Date.now() - started) / 1000)} s)`);
      notedAt = Date.now();
    }
    const v = await probe();
    if (v !== undefined) return v;
    if (Date.now() > until) throw new Error(`timed out after ${Math.round(ms / 1000)} s waiting for ${what}`);
    await sleep(every);
  }
}

const mainIdentity = async (s: Session, w: W): Promise<string | null> => {
  const st = await s.run<{ kind: string; main?: string }>("dashpay", "--wallet", idOf(w), "status");
  return st.kind === "ready" ? st.main! : null;
};

/** W's identity and name: started once (never while W has an identity or a registration row), then
 * followed to done. */
async function register(s: Session, w: W) {
  const wallet = idOf(w);
  let reg = state.registrations[w];
  if (reg?.state === "done") return;
  const rows = await s.run<Row[]>("identity", "--wallet", wallet, "registrations");
  let row = reg ? rows.find((r) => r.label === reg!.label) : rows[0];
  if (!row && reg?.state === "started") throw new Error(`${w}'s registration ${reg.draft} is gone from the engine: look before running again`);
  if (!row) {
    const ids = await s.run<unknown[]>("identity", "--wallet", wallet, "list");
    if (ids.length > 0) throw new Error(`${w} has an identity but fixture-state.json has no registration: look before running again`);
    // Spendable funds first: the quote's available set must cover the total.
    let lastQuote = "";
    const quote = await poll(REGISTRATION_TIMEOUT_MS, `${w}'s funds to be spendable`, 15_000, async () => {
      const q = (await s.run<{ quote: { available_duffs: number; total_duffs: number; lock_duffs: number; remaining_credits: number } }>(
        "identity", "--wallet", wallet, "register", "--label", "dwo4aquote2", "--quote-only",
      )).quote;
      const seen = `${q.available_duffs}/${q.total_duffs}`;
      if (seen !== lastQuote) console.log(`fixture: ${w}: spendable ${q.available_duffs} of the ${q.total_duffs} duffs the registration takes`);
      lastQuote = seen;
      return q.available_duffs >= q.total_duffs ? q : undefined;
    });
    const label = await (async () => {
      for (let i = 0; i < 5; i++) {
        const l = newLabel(w);
        const a = await s.run<{ kind: string; contested?: boolean }>("name", "--wallet", wallet, "availability", l);
        if (a.kind === "available" && !a.contested) return l;
      }
      throw new Error("five labels in a row were not available");
    })();
    reg = { state: "starting", label, quote: { lockDuffs: quote.lock_duffs, totalDuffs: quote.total_duffs, remainingCredits: quote.remaining_credits } };
    state.registrations[w] = reg;
    step(`${w}: registering @${label} (lock ${quote.lock_duffs}, total ${quote.total_duffs} duffs, ${quote.remaining_credits} credits left after)`);
    const started = await s.run<{ draft: string }>("identity", "--wallet", wallet, "register", "--label", label, "--display-name", `QA ${w}`);
    reg.state = "started";
    reg.draft = started.draft;
    step(`${w}: registration ${started.draft} started`);
    row = undefined;
  } else if (!reg) {
    reg = { state: "started", label: row.label, draft: row.draft };
    state.registrations[w] = reg;
    step(`${w}: found registration ${row.draft} (@${row.label}) in the engine`);
  }
  let last = "";
  const done = await poll(REGISTRATION_TIMEOUT_MS, `${w}'s registration to end`, 15_000, async () => {
    const r = (await s.run<Row[]>("identity", "--wallet", wallet, "registrations")).find((x) => x.label === reg!.label);
    if (!r) return undefined;
    const now = `${r.phase} waiting=${r.waiting} running=${r.running}`;
    if (now !== last) step(`${w}: ${now}${r.txid ? ` lock ${r.txid}` : ""}`);
    last = now;
    if (r.failure) throw new Error(`${w}'s registration failed: ${r.failure.code} at ${r.failure.phase}`);
    if (!r.running && (r.waiting === "unlock" || r.waiting === "authorize")) {
      step(`${w}: resuming ${r.draft} (waiting ${r.waiting})`);
      await s.run("identity", "--wallet", wallet, "resume", r.draft, "--max-duffs", String(reg!.quote?.totalDuffs ?? 0));
    }
    return r.phase === "done" ? r : undefined;
  });
  if (done.name_failure) throw new Error(`${w}'s identity registered but not its name: ${done.name_failure}`);
  const main = await poll(300_000, `${w}'s DashPay status to be ready`, 10_000, async () => (await mainIdentity(s, w)) ?? undefined);
  reg.state = "done";
  reg.identity = main;
  step(`${w}: identity ${main}, @${reg.label}`);
}

interface ContactRowInfo {
  contact: string;
  relation: string;
}

const relations = async (s: Session, w: W): Promise<ContactRowInfo[]> => {
  const l = await s.run<{ sections: { contacts: ContactRowInfo[] }[] }>("contact", "--wallet", idOf(w), "list");
  return l.sections.flatMap((x) => x.contacts);
};
const relationOf = async (s: Session, w: W, other: W) =>
  (await relations(s, w)).find((r) => r.contact === state.registrations[other]!.identity)?.relation ?? "stranger";

/** `from` asks `to`, `to` accepts: each write only when the relation says it is still needed. */
async function befriend(s: Session, from: W, to: W) {
  const key = `${from}>${to}`;
  if (state.contacts[key]?.state === "contacts") return;
  const toId = state.registrations[to]!.identity!;
  const fromId = state.registrations[from]!.identity!;
  await s.run("dashpay", "--wallet", idOf(from), "sync");
  let rel = await relationOf(s, from, to);
  if (rel === "stranger") {
    if (state.contacts[key]?.state === "requesting") throw new Error(`fixture-state.json says ${key} was being requested and ${from} shows no relation: look before running again`);
    state.contacts[key] = { state: "requesting", at: new Date().toISOString() };
    step(`${from} asks ${to} (${toId})`);
    const out = await s.run<{ kind: string }>("contact", "--wallet", idOf(from), "request", toId);
    state.contacts[key] = { state: "sent", at: new Date().toISOString() };
    step(`${from}→${to} request: ${out.kind}`);
    rel = await relationOf(s, from, to);
  } else if (rel !== "pending_outgoing" && rel !== "contact") {
    throw new Error(`${from} relates to ${to} as ${rel}`);
  }
  if (rel !== "contact") {
    const incoming = await poll(CONTACT_TIMEOUT_MS, `${to} to see ${from}'s request`, 20_000, async () => {
      await s.run("dashpay", "--wallet", idOf(to), "sync");
      const r = await relationOf(s, to, from);
      return r === "pending_incoming" || r === "contact" ? r : undefined;
    });
    if (incoming === "pending_incoming") {
      state.contacts[key] = { state: "accepting", at: new Date().toISOString() };
      step(`${to} accepts ${from} (${fromId})`);
      const out = await s.run<{ kind: string }>("contact", "--wallet", idOf(to), "accept", fromId);
      step(`${to} accepted ${from}: ${out.kind}`);
    }
    await poll(CONTACT_TIMEOUT_MS, `${from} to list ${to} as a contact`, 20_000, async () => {
      await s.run("dashpay", "--wallet", idOf(from), "sync");
      return (await relationOf(s, from, to)) === "contact" ? true : undefined;
    });
  }
  state.contacts[key] = { state: "contacts", at: new Date().toISOString() };
  step(`${from} and ${to} are contacts`);
}

// ------------------------------------------------------------------ main

async function main() {
  if (!(FIXTURE + "/").startsWith(homedir() + "/")) throw new Error(`${FIXTURE}: the fixture must lie under ${homedir()} (the secret store links only there)`);
  mkdirSync(FIXTURE, { recursive: true, mode: 0o700 });
  mkdirSync(LOGS, { recursive: true, mode: 0o700 });
  if (existsSync(STATE_FILE)) state = { ...state, ...(JSON.parse(readFileSync(STATE_FILE, "utf8")) as State) };
  secrets.push(passphrase());
  await wallets();
  await faucet();
  await shares();

  const s = new Session();
  let lastKind: string | undefined;
  try {
    await poll(300_000, "DashPay's bring-up", 10_000, async () => {
      const a = await s.ask("dashpay", "--wallet", idOf("A"), "status");
      if (!a.ok && a.error?.code === "session_exit") throw new Error(a.error.message);
      const kind = a.ok ? a.result?.kind : a.error?.code;
      if (kind !== lastKind) console.log(`fixture: DashPay status of A: ${kind}`);
      lastKind = kind;
      return a.ok && kind !== "startup_incomplete" ? true : undefined;
    });
    // Registrations run side by side: each waits for SPV, its lock's proof and Platform.
    const results = await Promise.allSettled(WALLETS.map((w) => register(s, w)));
    const failed = results.flatMap((r, i) => (r.status === "rejected" ? [`${WALLETS[i]}: ${r.reason instanceof Error ? r.reason.message : String(r.reason)}`] : []));
    if (failed.length) throw new Error(failed.join("; "));
    for (const [from, to] of EDGES) await befriend(s, from, to);
    for (const [x, y] of STRANGERS) {
      const r = await relationOf(s, x, y);
      if (r !== "stranger") throw new Error(`${x} and ${y} must not relate, but ${x} sees ${y} as ${r}`);
    }
    step(`not related, as the flows need: ${STRANGERS.map(([x, y]) => `${x}–${y}`).join(", ")}`);

    const fixture: Record<string, unknown> = {};
    for (const w of WALLETS) {
      const reg = state.registrations[w]!;
      const contacts = (await relations(s, w)).filter((r) => r.relation === "contact").map((r) => WALLETS.find((x) => state.registrations[x]?.identity === r.contact) ?? r.contact);
      const balance = await s.run<unknown>("identity", "--wallet", idOf(w), "balance").catch(() => null);
      fixture[w] = {
        wallet_id: idOf(w),
        identity_id: reg.identity,
        username: reg.label,
        expected_main_name: reg.label,
        birth_height: state.birthHeight,
        contacts: contacts.sort(),
        credits: balance,
      };
    }
    writeFileSync(join(FIXTURE, "fixture.json"), `${JSON.stringify(fixture, null, 2)}\n`, { mode: 0o600 });
    step(`fixture.json written: ${WALLETS.map((w) => `${w} @${state.registrations[w]!.label} {${(fixture[w] as { contacts: string[] }).contacts.join(",")}}`).join("; ")}`);
  } finally {
    await s.close();
  }
  const found = [LOGS, STATE_FILE, join(FIXTURE, "fixture.json")].flatMap((p) => (existsSync(p) && lstatSync(p).isDirectory() ? leaks(p, secrets) : []));
  for (const f of [STATE_FILE, join(FIXTURE, "fixture.json")]) if (existsSync(f) && secrets.some((x) => readFileSync(f, "utf8").includes(x))) found.push(f);
  if (found.length) throw new Error(`secrets in plain text: ${found.join(", ")}`);
  console.log("fixture: READY");
}

main().catch((e) => {
  const msg = mask(e instanceof Error ? e.message : String(e));
  console.error(`fixture: FAIL: ${msg}`);
  try {
    state.steps.push(`${new Date().toISOString()} FAIL: ${msg}`);
    save();
  } catch {
    // the state file is best effort here
  }
  process.exitCode = 1;
});
