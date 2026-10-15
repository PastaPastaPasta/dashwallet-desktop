// dw-fixture-module: never in a production build (vite.config.ts, noFixtureModules).
// The fixture's Tools (U-05): node facts and warnings, a console over a subset of dw-console's
// commands, peers, Repair (rescan, reset, birth height, drop unconfirmed, stuck funding), the
// address book, message signing, PSBTs, coin locks, and files from the OS dialog (dw-app's
// `api/files.rs`, D20). The texts and codes are the engine's (dw-console, dw-message, dw-app).

import type { AddressBookEntry } from "../api/generated/AddressBookEntry";
import type { ConsoleCommand } from "../api/generated/ConsoleCommand";
import type { FilePurpose } from "../api/generated/FilePurpose";
import type { GrantPurposeSpec } from "../api/generated/GrantPurposeSpec";
import type { NodeSummary } from "../api/generated/NodeSummary";
import type { NodeWarning } from "../api/generated/NodeWarning";
import type { OpenedFile } from "../api/generated/OpenedFile";
import type { OutPoint } from "../api/generated/OutPoint";
import type { PeerSummary } from "../api/generated/PeerSummary";
import type { PsbtAnalysis } from "../api/generated/PsbtAnalysis";
import type { PsbtOutput } from "../api/generated/PsbtOutput";
import type { RescanStatus } from "../api/generated/RescanStatus";
import type { VerifyMessageResult } from "../api/generated/VerifyMessageResult";
import type { AppError, AppEvent } from "../api/generated/commands";
import type { Ctx, Handlers } from "./handlers";
import type { FixtureGrant, FixtureWallet, World } from "./scenarios";
import { classifyAddress, owns } from "./send";
import { FIXTURE_DATA_DIR } from "./settings";
import { FIXTURE_NOW, makeIds, prng, secs, TIP } from "./wallet";

/** What the OS dialogs answer, and what was saved. */
export interface FilesWorld {
  /** The file the next open dialog picks; `null`: the user cancels. */
  open: OpenedFile | null;
  /** The folder a save dialog picks (the suggested name is kept); `null`: the user cancels. */
  saveDir: string | null;
  /** Every dialog shown, in order. */
  dialogs: { purpose: FilePurpose; save: boolean; suggestedName: string | null }[];
  /** Every file written, in order (data base64). */
  saved: { purpose: FilePurpose; path: string; data: string }[];
}

/** A PSBT the fixture knows, by its base64. Outputs' `isMine` is the signer's view. */
export interface FixturePsbt {
  outputs: PsbtOutput[];
  /** `null`: an input lacks its previous transaction. */
  fee: number | null;
  inputs: number;
  signedInputs: number;
  /** The wallet holding the inputs' keys; `null`: none of the fixture's. */
  signer: string | null;
}

export interface ToolsWorld {
  files: FilesWorld;
  warnings: NodeWarning[];
  peers: PeerSummary[];
  /** The running rescan; `null`: none. */
  rescan: RescanStatus | null;
  /** How many blocks each `rescan_progress` poll moves the rescan on. */
  rescanStep: number;
  /** How often `reset_chain_data` ran. */
  resets: number;
  addressBook: Record<string, AddressBookEntry[]>;
  psbts: Record<string, FixturePsbt>;
  /** Every PSBT broadcast, in order. */
  broadcasts: string[];
}

const B64 = (latin1: string) => btoa(latin1);
const PAYEE = "yPsbtPayee1111111111111111111111Ab";
const STARTED_SECS = secs(FIXTURE_NOW) - 3 * 3600;

/** The PSBTs a test can paste or load: one the main wallet can sign, one it cannot, and a fully
 * signed one. Binary (`psbt\xff…`) once decoded, so a file holds the same bytes. */
export const FIXTURE_PSBT = {
  unsigned: B64("psbt\xffunsigned"),
  foreign: B64("psbt\xffforeign"),
  complete: B64("psbt\xffcomplete"),
};

function newPeers(): PeerSummary[] {
  const rand = prng(0x9ee5);
  return Array.from({ length: 8 }, (_, i) => ({
    address: `203.0.113.${10 + i}:19999`,
    userAgent: "/Dash Core:23.1.2/",
    protocolVersion: 70237,
    bestHeight: TIP,
    pingMs: 20 + Math.floor(rand() * 180),
    connectedSince: STARTED_SECS + i * 600,
    inbound: false,
    bytesSent: 40_000 + Math.floor(rand() * 4_000_000),
    bytesReceived: 900_000 + Math.floor(rand() * 90_000_000),
  }));
}

/** The address book: two sending entries, and the wallet's labelled coins' addresses as its
 * receiving entries (the Payroll coin's address is labelled Payroll). */
function newAddressBook(main: FixtureWallet): AddressBookEntry[] {
  const { address } = makeIds(prng(0xb00c));
  const receiving = [...new Map(main.coins.filter((u) => u.label).map((u) => [u.address, u.label!])).entries()];
  return [
    { address: address(), label: "Alice", purpose: "send", createdAt: STARTED_SECS },
    { address: address(), label: "Bob's shop", purpose: "send", createdAt: STARTED_SECS },
    ...receiving.map(([a, label]): AddressBookEntry => ({ address: a, label, purpose: "receive", createdAt: null })),
  ];
}

export function newToolsWorld(main: FixtureWallet): ToolsWorld {
  const walletId = main.summary.walletId;
  const change = main.coins[0]?.address ?? PAYEE;
  return {
    files: { open: null, saveDir: "/home/user/Documents", dialogs: [], saved: [] },
    warnings: [],
    peers: newPeers(),
    rescan: null,
    rescanStep: 10_000,
    resets: 0,
    addressBook: { [walletId]: newAddressBook(main) },
    psbts: {
      [FIXTURE_PSBT.unsigned]: {
        outputs: [
          { address: PAYEE, amount: 150_000_000, isMine: false },
          { address: change, amount: 49_999_774, isMine: true },
        ],
        fee: 226,
        inputs: 2,
        signedInputs: 0,
        signer: walletId,
      },
      [FIXTURE_PSBT.foreign]: { outputs: [{ address: PAYEE, amount: 25_000_000, isMine: false }], fee: 1_000, inputs: 1, signedInputs: 0, signer: null },
      [FIXTURE_PSBT.complete]: { outputs: [{ address: PAYEE, amount: 10_000_000, isMine: false }], fee: 226, inputs: 1, signedInputs: 1, signer: walletId },
    },
    broadcasts: [],
  };
}

export interface ToolsDeps {
  wallet(w: World, walletId: string): FixtureWallet;
  /** An unused grant of the wallet, of one of `kinds`, that a key can use; or `null`. */
  grant(w: World, grantId: string, walletId: string, kinds: GrantPurposeSpec["kind"][]): FixtureGrant | null;
  /** Redeems (or revokes) a grant. */
  useGrant(w: World, grantId: string): void;
}

const fail = (code: AppError["code"], message: string, data: AppError["data"] = null): AppError => ({ code, message, data });
const rpc = (rpcCode: number, rpcMessage: string) => fail("console.rpc_error", rpcMessage, { kind: "rpc", rpcCode, rpcMessage });

const savePath = (files: FilesWorld, name: string | null) => (files.saveDir === null ? null : `${files.saveDir}/${name || "Untitled"}`);

const TIP_HASH = "00000ab1c3".padEnd(64, "7");

function nodeInfo(world: World): NodeSummary {
  const { sync, launch } = world;
  return {
    // Settings ▸ About and General show these two (U-04).
    userAgent: `/DashWallet:${launch.appVersion}/`,
    dataDir: FIXTURE_DATA_DIR,
    startupTime: STARTED_SECS,
    network: launch.network,
    connectionsIn: 0,
    connectionsOut: sync.running ? world.tools.peers.length : 0,
    localAddresses: [],
    tipHeight: sync.tipHeight,
    tipTime: sync.tipTime,
    tipHash: sync.tipHeight === null ? null : TIP_HASH,
    bestChainlock: sync.chainlockHeight === null ? null : { height: sync.chainlockHeight, blockHash: TIP_HASH, blockTime: sync.tipTime },
    masternodes: { total: 3_241, enabled: 3_187 },
    evonodes: { total: 182, enabled: 179 },
    // SPV keeps no mempool (QT-143: shown as "—").
    mempoolTxCount: null,
    mempoolUsageBytes: null,
  };
}

// ---------------------------------------------------------------- console

/** dash-qt's history filter (dw-console `SENSITIVE`). */
const SENSITIVE = [
  "importprivkey",
  "importmulti",
  "sethdseed",
  "signmessagewithprivkey",
  "signrawtransactionwithkey",
  "upgradetohd",
  "walletpassphrase",
  "walletpassphrasechange",
  "encryptwallet",
];

const command = (name: string, category: string, available: boolean, usage = ""): ConsoleCommand => ({
  name,
  category,
  sensitive: SENSITIVE.includes(name),
  available,
  usage,
});

/** A subset of dw-console's `COMMANDS`, with its categories and usage lines, sorted by name. */
export const CONSOLE_COMMANDS: ConsoleCommand[] = [
  command("dumpwallet", "Wallet", false),
  command("getbalance", "Wallet", true, "getbalance"),
  command("getbestblockhash", "Blockchain", false),
  command("getblock", "Blockchain", false),
  command("getblockcount", "Blockchain", true, "getblockcount"),
  command("getconnectioncount", "Network", true, "getconnectioncount"),
  command("getnetworkinfo", "Network", true, "getnetworkinfo"),
  command("help", "Control", true, 'help ( "command" )'),
  command("listwallets", "Wallet", true, "listwallets"),
  command("signmessage", "Wallet", true, 'signmessage "address" "message"'),
  command("stop", "Control", false),
  command("uptime", "Control", true, "uptime"),
  command("validateaddress", "Util", true, 'validateaddress "address"'),
  command("verifymessage", "Util", true, 'verifymessage "address" "signature" "message"'),
];

/** dw-console's `help-console` text. */
const HELP_CONSOLE =
  "\nThis console accepts RPC commands using the standard syntax.\n   example:    getblockhash 0\n\nThis console can also accept RPC commands using the parenthesized syntax.\n   example:    getblockhash(0)\n\nCommands may be nested when specified with the parenthesized syntax.\n   example:    getblock(getblockhash(0) 1)\n\nA space or a comma can be used to delimit arguments for either syntax.\n   example:    getblockhash 0\n               getblockhash,0\n\nNamed results can be queried with a non-quoted key string in brackets using the parenthesized syntax.\n   example:    getblock(getblockhash(0) 1)[tx]\n\nResults without keys can be queried with an integer in brackets using the parenthesized syntax.\n   example:    getblock(getblockhash(0),1)[tx][0]\n\n";

/** The line's method and arguments: the standard syntax, or one flat parenthesized call. Nested
 * calls and `[key]` indexing are dw-console's and not modelled. */
function parseLine(line: string): [string, string[]] {
  const t = line.trim();
  const call = /^([A-Za-z_-]+)\s*\((.*)\)$/.exec(t);
  const [head, rest] = call ? [call[1], call[2]] : [t.split(/[\s,]/, 1)[0], t.slice(t.split(/[\s,]/, 1)[0].length)];
  if ((rest.match(/"/g)?.length ?? 0) % 2 || /[()]/.test(rest)) throw fail("console.parse_error", "Parse error: unbalanced ' or \"");
  const args = [...rest.matchAll(/"([^"]*)"|([^\s,]+)/g)].map((m) => m[1] ?? m[2]);
  return [head, args];
}

/** dw-console `redact`: a sensitive command's arguments become `(…)`. */
export function redact(line: string): string {
  const [method] = parseLine(line);
  return SENSITIVE.includes(method.toLowerCase()) ? `${line.trim().slice(0, method.length)}(…)` : line;
}

const json = (value: unknown) => ({ text: JSON.stringify(value, null, 2), isJson: true });
const plain = (text: string | number) => ({ text: String(text), isJson: false });
const hash = (text: string) => [...text].reduce((h, c) => (Math.imul(h, 31) + c.charCodeAt(0)) >>> 0, 7);

/** A 65-byte compact signature, base64 as Core's: the same for an address and message every run. */
function signature(address: string, message: string): string {
  const rand = prng(hash(`${address}\n${message}`));
  return B64(String.fromCharCode(0x1f, ...Array.from({ length: 64 }, () => Math.floor(rand() * 256))));
}

function verify(world: World, address: string, sig: string, message: string): VerifyMessageResult {
  const kind = classifyAddress(world.launch.network.dirName, address);
  if (kind.kind !== "core") return { outcome: "invalidAddress", text: "Invalid address: check it and try again." };
  if (kind.scriptHash) return { outcome: "addressNoKey", text: "That address is a script, not a key: check it and try again." };
  if (!/^[A-Za-z0-9+/]{86,88}={0,2}$/.test(sig))
    return { outcome: "malformedSignature", text: "The signature could not be decoded. Please check the signature and try again." };
  if (sig !== signature(address, message)) return { outcome: "notSigned", text: "Message verification failed." };
  return { outcome: "verified", text: "Message verified." };
}

// ---------------------------------------------------------------- PSBT

function psbtOf(world: World, psbt: string): FixturePsbt {
  const p = world.tools.psbts[psbt];
  if (!p) throw fail("psbt.invalid", "PSBT decode failed");
  return p;
}

function analyze(world: World, deps: ToolsDeps, walletId: string | null, p: FixturePsbt): PsbtAnalysis {
  const watchOnly = walletId !== null && deps.wallet(world, walletId).summary.watchOnly;
  const outputs = p.outputs.map((o) => ({ ...o, isMine: o.isMine && walletId === p.signer }));
  const externalSent = outputs.filter((o) => !o.isMine).reduce((n, o) => n + o.amount, 0);
  return {
    outputs,
    fee: p.fee,
    total: p.fee === null ? null : externalSent + p.fee,
    unsignedInputs: p.inputs - p.signedInputs,
    status: p.fee === null ? "missingInputInfo" : p.signedInputs === p.inputs ? "complete" : "needsSignatures",
    signability: walletId === null ? "noWallet" : watchOnly ? "watchOnly" : walletId === p.signer ? "canSign" : "noMatchingKeys",
    externalSent: p.fee === null ? null : externalSent,
  };
}

/** A fixture txid for a PSBT: the same one for its bytes every run. */
const txidOf = (psbt: string) => makeIds(prng(hash(psbt))).hex(64);

// ---------------------------------------------------------------- handlers

function syncChanged({ world, emit }: Ctx) {
  const event: AppEvent = { kind: "sync", network: world.launch.network.dirName, status: world.sync };
  emit(event);
}

function startRescan(world: World, fromHeight: number) {
  world.tools.rescan = { fromHeight, currentHeight: fromHeight, targetHeight: world.sync.tipHeight ?? TIP, startedAt: secs(FIXTURE_NOW) };
}

const book = (world: World, walletId: string) => (world.tools.addressBook[walletId] ??= []);

export function toolsHandlers(deps: ToolsDeps): Handlers {
  const { wallet, grant, useGrant } = deps;

  /** dw-console's executor for the fixture's commands. */
  function execute(world: World, walletId: string | null, line: string, grantId: string | null) {
    if (line.trim() === "help-console") return plain(HELP_CONSOLE);
    const [method, args] = parseLine(line);
    const spec = CONSOLE_COMMANDS.find((c) => c.name === method);
    if (!spec?.available) throw fail("console.not_available", `Method not available: ${method}`);
    const arity = (min: number, max: number) => {
      if (args.length < min || args.length > max) throw rpc(-1, spec.usage);
    };
    const own = () => {
      if (walletId === null) throw fail("console.wallet_required", "no wallet selected");
      return wallet(world, walletId);
    };
    switch (method) {
      case "help": {
        arity(0, 1);
        if (args[0]) return plain(CONSOLE_COMMANDS.find((c) => c.name === args[0] && c.available)?.usage || `help: unknown command: ${args[0]}`);
        const categories = [...new Set(CONSOLE_COMMANDS.filter((c) => c.available).map((c) => c.category))].sort();
        return plain(
          categories
            .map((cat) => `== ${cat} ==\n${CONSOLE_COMMANDS.filter((c) => c.available && c.category === cat).map((c) => c.usage).join("\n")}`)
            .join("\n\n"),
        );
      }
      case "getblockcount":
        arity(0, 0);
        if (world.sync.tipHeight === null) throw rpc(-1, "Block height is not known yet");
        return plain(world.sync.tipHeight);
      case "getconnectioncount":
        arity(0, 0);
        return plain(world.sync.connectedPeers);
      case "uptime":
        arity(0, 0);
        return plain(secs(FIXTURE_NOW) - STARTED_SECS);
      case "getnetworkinfo": {
        arity(0, 0);
        const info = nodeInfo(world);
        return json({
          subversion: info.userAgent,
          networkactive: world.sync.running,
          connections: info.connectionsIn + info.connectionsOut,
          connections_in: info.connectionsIn,
          connections_out: info.connectionsOut,
          localaddresses: [],
        });
      }
      case "listwallets":
        arity(0, 0);
        return json(world.wallets.map((w) => w.summary.name));
      case "getbalance":
        arity(0, 4);
        return plain(((own().summary.balances?.confirmed ?? 0) / 1e8).toFixed(8));
      case "validateaddress": {
        arity(1, 1);
        const kind = classifyAddress(world.launch.network.dirName, args[0]);
        return json(kind.kind === "core" ? { isvalid: true, address: args[0], isscript: kind.scriptHash } : { isvalid: false });
      }
      case "verifymessage": {
        arity(3, 3);
        const r = verify(world, args[0], args[1], args[2]);
        if (r.outcome === "invalidAddress") throw rpc(-5, "Invalid address");
        if (r.outcome === "addressNoKey") throw rpc(-3, "Address does not refer to key");
        if (r.outcome === "malformedSignature") throw rpc(-5, "Malformed base64 encoding");
        return plain(String(r.outcome === "verified"));
      }
      case "signmessage": {
        arity(2, 2);
        const w = own();
        // The grant answers the request this line made once without one.
        if (grantId === null) throw fail("console.authorization_required", "authorization required", { kind: "authorize", purpose: { kind: "signMessage" }, walletId });
        if (!grant(world, grantId, w.summary.walletId, ["signMessage"])) throw fail("vault.grant_invalid", "grant invalid");
        useGrant(world, grantId);
        return plain(signMessage(world, w, args[0], args[1]));
      }
    }
    throw fail("console.not_available", `Method not available: ${method}`);
  }

  function signMessage(world: World, w: FixtureWallet, address: string, message: string): string {
    const kind = classifyAddress(world.launch.network.dirName, address);
    if (kind.kind !== "core") throw fail("invalid_address", "invalid address");
    if (kind.scriptHash) throw fail("address_no_key", "a P2SH address has no key");
    if (!owns(w, address)) throw fail("address_not_mine", "address not mine");
    return signature(address, message);
  }

  return {
    // Information
    node_info: (_, { world }) => nodeInfo(world),
    warnings: (_, { world }) => world.tools.warnings,

    // Console
    console_commands: () => CONSOLE_COMMANDS,
    console_redact: ({ line }) => redact(line),
    console_execute: ({ walletId, line, grantId }, { world }) => ({ ...execute(world, walletId, line, grantId), filtered: redact(line) }),

    // Peers
    peers: (_, { world }) => (world.sync.running ? world.tools.peers : []),
    rotate_peers: (_, { world }) => {
      if (!world.sync.running || world.tools.peers.length === 0) throw fail("no_peers", "no peers");
      world.tools.peers = world.tools.peers.map((p, i) => ({ ...p, address: `198.51.100.${20 + i}:19999`, connectedSince: secs(FIXTURE_NOW), bytesSent: 0, bytesReceived: 0 }));
      return null;
    },

    // Repair (E0-04.P2b)
    spv_running: (_, { world }) => world.sync.running,
    stop_spv: (_, ctx) => {
      ctx.world.sync = { ...ctx.world.sync, running: false, connectedPeers: 0 };
      ctx.world.tools.rescan = null;
      syncChanged(ctx);
      return null;
    },
    start_spv: (_, ctx) => {
      ctx.world.sync = { ...ctx.world.sync, running: true, connectedPeers: ctx.world.tools.peers.length };
      syncChanged(ctx);
      return null;
    },
    reset_chain_data: (_, { world }) => {
      if (world.sync.running) throw fail("spv_running", "SPV is running");
      world.tools.resets += 1;
      return null;
    },
    rescan: ({ from }, { world }) => {
      if (!world.sync.running) throw fail("spv_not_running", "SPV is not running");
      if (world.tools.rescan) throw fail("rescan_in_progress", "a rescan is running");
      const tip = world.sync.tipHeight ?? TIP;
      const height = from.kind === "genesis" ? 0 : from.kind === "height" ? from.height : Math.min(...world.wallets.map((w) => w.summary.birthHeight ?? 0));
      if (height > tip) throw fail("height_out_of_range", "height above the tip");
      startRescan(world, height);
      return null;
    },
    // Each poll moves the rescan on by `rescanStep`; it ends at the tip.
    rescan_progress: (_, { world }) => {
      const r = world.tools.rescan;
      if (!r) return null;
      const current = Math.min((r.currentHeight ?? r.fromHeight) + world.tools.rescanStep, r.targetHeight ?? TIP);
      world.tools.rescan = current >= (r.targetHeight ?? TIP) ? null : { ...r, currentHeight: current };
      return r;
    },
    cancel_rescan: (_, { world }) => {
      const running = world.tools.rescan !== null;
      world.tools.rescan = null;
      return running;
    },
    set_birth_height: ({ walletId, height }, { world }) => {
      const w = wallet(world, walletId);
      if (height > (world.sync.tipHeight ?? TIP)) throw fail("height_out_of_range", "height above the tip");
      const lower = height < (w.summary.birthHeight ?? Infinity);
      w.summary = { ...w.summary, birthHeight: height };
      // IOS-113: a lower height schedules a rescan from it.
      if (lower && world.sync.running && !world.tools.rescan) startRescan(world, height);
      return null;
    },
    // Counts the unconfirmed coins that are not InstantSend-locked; they stay listed (not modelled).
    drop_unconfirmed: ({ walletId }, { world }) => {
      const ws = walletId === null ? world.wallets : [wallet(world, walletId)];
      const dropped = ws.flatMap((w) => w.coins).filter((u) => u.confirmations === 0 && !u.instantLocked).length;
      if (dropped && world.sync.running && !world.tools.rescan) startRescan(world, Math.min(...ws.map((w) => w.summary.birthHeight ?? 0)));
      return dropped;
    },
    resolve_stuck_funding: ({ walletId, step, grant: grantId }, { world }) => {
      wallet(world, walletId);
      const draft = /^registration\/(.+)\/funding$/.exec(step)?.[1];
      const r = world.dashpay.registrations.find((x) => x.draft === draft);
      if (!r || r.waiting !== "fundingUnknown") throw fail("stuck_funding", "funding state is known", { kind: "stuckFunding", refusal: { kind: "notStuck", status: null }, retryLater: false });
      if (!grant(world, grantId, walletId, ["spend"])) throw fail("vault.grant_invalid", "grant invalid");
      useGrant(world, grantId);
      r.waiting = null;
      return { txid: txidOf(step) };
    },

    // Address book (QT-095…098)
    address_book: ({ walletId, purpose }, { world }) => {
      wallet(world, walletId);
      return book(world, walletId).filter((e) => purpose === null || e.purpose === purpose);
    },
    save_address_book_entry: ({ walletId, address, label, purpose, replace }, { world }) => {
      const w = wallet(world, walletId);
      const entries = book(world, walletId);
      const known = entries.find((e) => e.address === address);
      if (replace) {
        if (!known) throw fail("labels.entry_not_found", "entry not found");
        const edited = { ...known, label };
        world.tools.addressBook[walletId] = entries.map((e) => (e === known ? edited : e));
        return edited;
      }
      if (classifyAddress(world.launch.network.dirName, address).kind !== "core") throw fail("labels.invalid_address", "invalid address");
      if (known) throw fail("labels.duplicate_address", "duplicate entry");
      if (purpose === "send" && owns(w, address)) throw fail("labels.own_address", "the address is one of the wallet's own");
      const entry: AddressBookEntry = { address, label, purpose, createdAt: secs(FIXTURE_NOW) };
      entries.push(entry);
      return entry;
    },
    delete_address_book_entry: ({ walletId, address }, { world }) => {
      wallet(world, walletId);
      const entries = book(world, walletId);
      const known = entries.find((e) => e.address === address);
      if (!known) throw fail("labels.entry_not_found", "entry not found");
      if (known.purpose === "receive") throw fail("labels.receive_entry_not_deletable", "receiving entries cannot be deleted");
      world.tools.addressBook[walletId] = entries.filter((e) => e !== known);
      return null;
    },

    // Sign/Verify (QT-099, QT-100)
    sign_message: ({ walletId, address, message, grantId }, { world }) => {
      const w = wallet(world, walletId);
      if (!grant(world, grantId, walletId, ["signMessage"])) throw fail("vault.grant_invalid", "grant invalid");
      useGrant(world, grantId);
      return signMessage(world, w, address, message);
    },
    verify_message: ({ address, signature: sig, message }, { world }) => verify(world, address, sig, message),

    // PSBT (QT-076…079)
    analyze_psbt: ({ walletId, psbt }, { world }) => analyze(world, deps, walletId, psbtOf(world, psbt)),
    // Signs every input the wallet holds the key of, under a `spend` grant covering the total.
    sign_psbt: ({ walletId, psbt, grantId }, { world }) => {
      const p = psbtOf(world, psbt);
      const a = analyze(world, deps, walletId, p);
      const g = grant(world, grantId, walletId, ["spend"]);
      if (!g) throw fail("vault.grant_invalid", "grant invalid");
      useGrant(world, grantId);
      if (a.signability === "watchOnly") throw fail("psbt.watch_only", "the wallet holds no private keys");
      if (a.total === null) throw fail("psbt.fee_unknown", "fee unknown");
      const cap = g.grant.purpose.kind === "spend" ? g.grant.purpose.maxDuffs : 0;
      if (a.total > cap) throw fail("psbt.grant_exceeded", "grant exceeded", { kind: "spendCap", maxDuffs: cap, outflow: a.total });
      if (a.signability !== "canSign" || p.signedInputs === p.inputs) return psbt;
      const signed = B64(atob(psbt) + "/signed");
      world.tools.psbts[signed] = { ...p, signedInputs: p.inputs };
      return signed;
    },
    broadcast_psbt: ({ psbt }, { world }) => {
      const p = psbtOf(world, psbt);
      if (p.signedInputs < p.inputs) throw fail("psbt.not_complete", "inputs left unsigned");
      if (!world.sync.running) throw fail("psbt.no_peers", "no peers");
      world.tools.broadcasts.push(psbt);
      return txidOf(psbt);
    },

    // Coin Selection (QT-070): user locks; a locked coin cannot be spent until unlocked.
    lock_outpoints: ({ walletId, outpoints }, { world }) => {
      setLocks(wallet(world, walletId), outpoints, true);
      return null;
    },
    unlock_outpoints: ({ walletId, outpoints }, { world }) => {
      setLocks(wallet(world, walletId), outpoints, false);
      return null;
    },

    // Files from the OS dialogs (D2)
    open_file: ({ purpose }, { world }) => {
      const { files } = world.tools;
      files.dialogs.push({ purpose, save: false, suggestedName: null });
      return files.open;
    },
    save_file_as: ({ purpose, suggestedName, data }, { world }) => {
      const { files } = world.tools;
      files.dialogs.push({ purpose, save: true, suggestedName });
      const path = savePath(files, suggestedName);
      if (path !== null) files.saved.push({ purpose, path, data });
      return path;
    },
    choose_file_path: ({ purpose, save, suggestedName }, { world }) => {
      const { files } = world.tools;
      files.dialogs.push({ purpose, save, suggestedName });
      return save ? savePath(files, suggestedName) : files.open && `${files.saveDir ?? "/home/user"}/${files.open.name}`;
    },
  };
}

const sameOutpoint = (a: OutPoint, b: OutPoint) => a.txid === b.txid && a.vout === b.vout;

function setLocks(w: FixtureWallet, outpoints: OutPoint[], locked: boolean) {
  for (const u of w.coins)
    if (outpoints.some((o) => sameOutpoint(o, u.outpoint)) && !u.reserved)
      Object.assign(u, { userLocked: locked, spendable: !locked && u.confirmations > 0 });
}
