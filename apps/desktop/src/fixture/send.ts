// dw-fixture-module: never in a production build (vite.config.ts, noFixtureModules).
// The fixture's send and receive engine (U-03 D18): coins, drafts, the estimate, prepare and
// broadcast, the transaction actions, receive addresses and requests, and the text helpers
// (classify_address, parse_amount, build_payment_uri, qr_matrix). It is not dw-engine's coin
// selection: inputs are taken largest first and the fee is the P2PKH size at a flat rate
// (`size = 10 + 148 × inputs + 34 × outputs` bytes). The real rules are dw-engine's, dw-units'
// and dw-uri's, tested in Rust; this models their answers and refusals for the screens.
// `World.send.broadcast` picks what the next broadcast does.

import type { AddressClass } from "../api/generated/AddressClass";
import type { AddressInfo } from "../api/generated/AddressInfo";
import type { AppError } from "../api/generated/AppError";
import type { CoinSource } from "../api/generated/CoinSource";
import type { ChangePolicy } from "../api/generated/ChangePolicy";
import type { DisplayUnit } from "../api/generated/DisplayUnit";
import type { FeeMode } from "../api/generated/FeeMode";
import type { FeePolicy } from "../api/generated/FeePolicy";
import type { OutPoint } from "../api/generated/OutPoint";
import type { PreparedTxSummary } from "../api/generated/PreparedTxSummary";
import type { QrMatrix } from "../api/generated/QrMatrix";
import type { ReceiveRequest } from "../api/generated/ReceiveRequest";
import type { Recipient } from "../api/generated/Recipient";
import type { TxActionRefusalName } from "../api/generated/TxActionRefusalName";
import type { TxDetailExtrasInfo } from "../api/generated/TxDetailExtrasInfo";
import type { TxEstimate } from "../api/generated/TxEstimate";
import type { TxRow } from "../api/generated/TxRow";
import type { Utxo } from "../api/generated/Utxo";
import type { Ctx, Handlers } from "./handlers";
import type { FixtureGrant, FixtureWallet, World } from "./scenarios";
import { mainUsername } from "./dashpay";
import { BASE58, FIXTURE_NOW, makeIds, prng, secs, TIP, txDetails, type HistoryVariant } from "./wallet";

const fail = (code: AppError["code"], message: string, data: AppError["data"] = null): AppError => ({ code, message, data });

// dw-engine send/mod.rs and fees.rs.
const MIN_FEE_PER_KB = 1_000;
const MAX_FEE_PER_KB = 10_000_000;
const MAX_TX_FEE = 10_000_000;
const MAX_BROADCAST_RATE_PER_KB = 10_000_000;
const FEE_TARGETS = [2, 4, 6, 12, 24, 48, 144, 504, 1008];
/** The P2PKH dust threshold (Recipient.amount). */
const DUST = 546;
const MAX_MONEY = 21_000_000 * 100_000_000;
const GAP_LIMIT = 20;
/** dw-uri's QR limit (QT-084). */
const QR_MAX_CHARS = 255;

/** What the next `draft_broadcast` does. */
export type BroadcastMode = "ok" | "unknown" | "rejected" | "noPeers";

interface Draft {
  walletId: string;
  recipients: Recipient[];
  source: CoinSource;
  fee: FeeMode;
  change: ChangePolicy;
}

interface Prepared {
  draft: number;
  walletId: string;
  summary: PreparedTxSummary;
  recipient: Recipient;
  /** A broadcast's outcome was unknown: it may be on the network, so it is never released. */
  handedOver: boolean;
}

export interface SendWorld {
  drafts: Record<number, Draft>;
  prepared: Record<number, Prepared>;
  /** The last draft or prepared handle issued. */
  handles: number;
  broadcast: BroadcastMode;
  /** The fixture's own sends: what an abandon gives back. */
  sent: Record<string, { walletId: string; spent: Utxo[]; change: OutPoint | null }>;
  /** `resend_transaction`'s announcements. */
  announced: Record<string, number>;
}

export const newSendWorld = (): SendWorld => ({ drafts: {}, prepared: {}, handles: 0, broadcast: "ok", sent: {}, announced: {} });

/** A wallet's receive side: the first unused receiving index, the last one issued, requests. */
export interface ReceiveWorld {
  firstUnused: number;
  issued: number;
  requests: ReceiveRequest[];
  nextRequest: number;
}

export const newReceiveWorld = (): ReceiveWorld => ({ firstUnused: 0, issued: 0, requests: [], nextRequest: 1 });

/** The funded wallet's coins: the confirmed ones add up to `FUNDED_BALANCES.confirmed`; the
 * unconfirmed one (neither mined nor InstantSend-locked) cannot be spent yet. */
export function buildCoins(variant: HistoryVariant): Utxo[] {
  if (variant === "empty") return [];
  const { hex, address } = makeIds(prng(0x5e17));
  const coin = (amount: number, confirmations: number, extra: Partial<Utxo> = {}): Utxo => ({
    outpoint: { txid: hex(64), vout: 0 },
    address: address(),
    amount,
    confirmations,
    blockHeight: confirmations > 0 ? TIP - confirmations + 1 : null,
    timestamp: secs(FIXTURE_NOW) - confirmations * 150,
    instantLocked: confirmations > 0,
    chainLocked: confirmations > 0,
    userLocked: false,
    reserved: false,
    label: null,
    isChange: false,
    isCoinbase: false,
    coinjoinDenominated: false,
    coinjoinRounds: null,
    spendable: confirmations > 0,
    ...extra,
  });
  return [
    coin(2_000_000_000, 150, { label: "Payroll" }),
    coin(750_000_000, 40),
    coin(250_000_000, 12, { isChange: true }),
    coin(19_698_287, 6),
    coin(20_000_000, 0),
  ];
}

// ---------------------------------------------------------------- text helpers

const BASE58_RE = new RegExp(`^[${BASE58}]+$`);

/** dw-uri's `classify` for the fixture's address shapes: Base58 P2PKH/P2SH of the network
 * (`y`/`8` off mainnet, `X`/`7` on it), and Platform's bech32m HRPs. */
export function classifyAddress(network: string, text: string): AddressClass {
  const t = text.trim();
  if (/^t?dash1[02-9ac-hj-np-z]+$/.test(t)) return { kind: "platform" };
  if (!BASE58_RE.test(t)) return { kind: "invalid", problem: { kind: "notBech32mOrBase58" } };
  if (t.length !== 34) return { kind: "invalid", problem: { kind: "invalidBase58Length" } };
  const [p2pkh, p2sh] = network === "mainnet" ? ["X", "7"] : ["y", "8"];
  if (t[0] !== p2pkh && t[0] !== p2sh) return { kind: "invalid", problem: { kind: "invalidBase58Prefix" } };
  return { kind: "core", scriptHash: t[0] === p2sh };
}

const DECIMALS: Record<DisplayUnit, number> = { dash: 8, milliDash: 5, microDash: 2, duffs: 0 };

/** `BitcoinUnits::parse`: spaces ignored, at most the unit's decimals, no sign. */
export function parseAmount(text: string, unit: DisplayUnit): number | null {
  const t = text.replace(/[\s ]/g, "");
  const decimals = DECIMALS[unit];
  const parts = t.split(".");
  if (t === "" || parts.length > 2) return null;
  const [whole, frac = ""] = parts;
  if (frac.length > decimals || !/^\d*$/.test(whole) || !/^\d*$/.test(frac) || whole.length + decimals > 18) return null;
  const digits = whole + frac.padEnd(decimals, "0");
  return digits === "" ? null : Number(digits);
}

/** Qt's `QUrl::toPercentEncoding`: everything but RFC 3986's unreserved characters. */
const percentEncode = (s: string) => encodeURIComponent(s).replace(/[!'()*]/g, (c) => `%${c.charCodeAt(0).toString(16).toUpperCase()}`);

/** dw-uri's `format_request_uri`: dash-qt's `formatBitcoinURI`, then `user=` last when `user`
 * is set (DP3-03, iOS's `PaymentURIBuilder`). */
export function paymentUri(address: string, amount: number | null, label: string | null, message: string | null, user: string | null = null): string {
  if (amount !== null && (amount < 0 || amount > MAX_MONEY)) throw fail("invalid_argument", "amount out of range");
  const fields: [string, string | null][] = [
    ["amount", amount === null ? null : (amount / 1e8).toFixed(8).replace(/\.?0+$/, "")],
    ["label", label && percentEncode(label)],
    ["message", message && percentEncode(message)],
  ];
  const query = fields.filter(([, v]) => v).map(([k, v]) => `${k}=${v}`);
  if (user) query.push(`user=${percentEncode(user)}`);
  return `dash:${address}${query.length ? `?${query.join("&")}` : ""}`;
}

/** A stored request as a read shows it: `username` and its `uri` follow the current main name. */
const withUsername = (r: ReceiveRequest, username: string | null): ReceiveRequest => ({
  ...r,
  username,
  uri: paymentUri(r.address, r.amount, r.label, r.message, username),
});

/** A stand-in for dw-uri's QR encoder: the size of the QR version the text needs, the three
 * finder patterns and timing lines, and modules drawn from the text. Not scannable (D18). */
export function qrMatrix(text: string): QrMatrix {
  if ([...text].length > QR_MAX_CHARS) throw fail("uri.too_long_for_qr", "resulting URI too long");
  const version = Math.min(10, 1 + Math.ceil(text.length / 20));
  const size = 17 + 4 * version;
  let seed = 0;
  for (const ch of text) seed = (Math.imul(seed, 31) + ch.charCodeAt(0)) >>> 0;
  const rand = prng(seed);
  const finder = (r: number, c: number) => {
    for (const [r0, c0] of [[0, 0], [0, size - 7], [size - 7, 0]]) {
      const y = r - r0;
      const x = c - c0;
      if (y >= -1 && y <= 7 && x >= -1 && x <= 7) {
        if (y < 0 || y > 6 || x < 0 || x > 6) return false;
        const ring = Math.min(y, x, 6 - y, 6 - x);
        return ring !== 1;
      }
    }
    return null;
  };
  const modules = Array.from({ length: size * size }, (_, i) => {
    const r = Math.floor(i / size);
    const c = i % size;
    const f = finder(r, c);
    if (f !== null) return f;
    if (r === 6 || c === 6) return (r + c) % 2 === 0;
    return rand() < 0.5;
  });
  return { size, modules };
}

// ---------------------------------------------------------------- coins and the estimate

const sizeOf = (inputs: number, outputs: number) => 10 + 148 * inputs + 34 * outputs;
const feeFor = (bytes: number, perKb: number) => Math.ceil((bytes * perKb) / 1000);
const rateOf = (fee: FeeMode) => (fee.kind === "perKb" ? fee.duffsPerKb : MIN_FEE_PER_KB);
const sameOutpoint = (a: OutPoint, b: OutPoint) => a.txid === b.txid && a.vout === b.vout;

export function feePolicy(): FeePolicy {
  return {
    source: "minimumRelay",
    minRelayPerKb: MIN_FEE_PER_KB,
    maxCustomPerKb: MAX_FEE_PER_KB,
    maxTxFee: MAX_TX_FEE,
    maxBroadcastRatePerKb: MAX_BROADCAST_RATE_PER_KB,
    targets: FEE_TARGETS.map((targetBlocks) => ({ targetBlocks, duffsPerKb: MIN_FEE_PER_KB })),
  };
}

function checkRate(fee: FeeMode) {
  if (fee.kind === "perKb" && (fee.duffsPerKb < MIN_FEE_PER_KB || fee.duffsPerKb > MAX_FEE_PER_KB))
    throw fail("invalid_argument", `fee rate out of range: ${fee.duffsPerKb}`);
}

/** The coins `source` may spend, largest first. */
function sourceCoins(wallet: FixtureWallet, source: CoinSource): Utxo[] {
  const coins = wallet.coins.filter((u) => u.spendable);
  return pickCoins(coins, source).toSorted((a, b) => b.amount - a.amount);
}

function pickCoins(coins: Utxo[], source: CoinSource): Utxo[] {
  switch (source.kind) {
    case "any":
      return coins;
    case "fullyMixedOnly":
      return coins.filter((u) => u.coinjoinRounds !== null);
    case "outpoints":
      return source.outpoints.map((o) => {
        const u = coins.find((x) => sameOutpoint(x.outpoint, o));
        if (!u) throw fail("send.outpoint_unavailable", "coin unavailable", { kind: "outpoint", txid: o.txid, vout: o.vout });
        return u;
      });
  }
}

interface Built extends TxEstimate {
  inputs: Utxo[];
}

/** The draft's transaction: its checks in dw-engine's order, then the inputs, fee and change. */
function build(world: World, wallet: FixtureWallet, d: Draft): Built {
  if (wallet.summary.watchOnly) throw fail("send.watch_only", "the wallet has no keys");
  if (d.recipients.length === 0) throw fail("send.no_recipients", "no recipients");
  const network = world.launch.network.dirName;
  d.recipients.forEach((r, index) => {
    const cls = classifyAddress(network, r.address);
    if (cls.kind === "platform") throw fail("send.platform_address", "platform address", { kind: "recipient", index });
    if (cls.kind !== "core") throw fail("send.invalid_address", "invalid address", { kind: "recipient", index });
    if (r.amount <= 0 || r.amount > MAX_MONEY) throw fail("send.invalid_amount", "invalid amount", { kind: "recipient", index });
    if (r.amount < DUST) throw fail("send.dust_amount", "dust amount", { kind: "recipient", index });
  });
  if (new Set(d.recipients.map((r) => r.address)).size !== d.recipients.length) throw fail("send.duplicate_address", "duplicate address");
  if (d.change.kind === "address" && classifyAddress(network, d.change.address).kind !== "core")
    throw fail("send.invalid_change_address", "invalid change address");
  checkRate(d.fee);
  const rate = rateOf(d.fee);
  const pay = d.recipients.reduce((n, r) => n + r.amount, 0);
  const subtract = d.recipients.some((r) => r.subtractFeeFromAmount);
  const coins = sourceCoins(wallet, d.source);
  const available = coins.reduce((n, u) => n + u.amount, 0);
  const outputs = d.recipients.length;
  if (pay > available) {
    if (d.source.kind === "fullyMixedOnly") throw fail("send.insufficient_mixed_funds", "not enough mixed funds", { kind: "duffs", needed: pay, available });
    throw fail("send.amount_exceeds_balance", "amount exceeds balance", { kind: "duffs", needed: pay, available });
  }
  // Coin control spends every selected coin; otherwise the largest coins until they cover it.
  const feeWith = (inputs: number, change: boolean) => feeFor(sizeOf(inputs, outputs + (change ? 1 : 0)), rate);
  const inputs: Utxo[] = [];
  let sum = 0;
  for (const u of coins) {
    inputs.push(u);
    sum += u.amount;
    if (d.source.kind !== "outpoints" && sum >= (subtract ? pay : pay + feeWith(inputs.length, true))) break;
  }
  // `base`: the transaction's fee at the rate, which a subtract-fee recipient pays.
  let base = feeWith(inputs.length, true);
  let change: number | null = sum - pay - (subtract ? 0 : base);
  let rest = 0;
  // Change below dust is not made: the transaction is smaller and the rest goes to the fee.
  if (change < DUST) {
    base = feeWith(inputs.length, false);
    rest = sum - pay - (subtract ? 0 : base);
    if (rest < 0) {
      if (d.source.kind === "fullyMixedOnly") throw fail("send.insufficient_mixed_funds", "not enough mixed funds", { kind: "duffs", needed: pay + base, available });
      throw fail("send.amount_with_fee_exceeds_balance", "amount with fee exceeds balance", { kind: "fee", fee: base, available });
    }
    change = null;
  }
  const fee = base + rest;
  const totalSent = subtract ? pay - base : pay;
  if (subtract && totalSent < DUST) throw fail("send.amount_too_small_after_fee", "amount too small after the fee", { kind: "recipient", index: 0 });
  if (fee > MAX_TX_FEE) throw fail("send.absurd_fee", "absurd fee", { kind: "fee", fee, available: null });
  return { fee, sizeBytes: sizeOf(inputs.length, outputs + (change === null ? 0 : 1)), inputCount: inputs.length, change, totalSent, inputs };
}

function maxSpendable(wallet: FixtureWallet, source: CoinSource, fee: FeeMode): number {
  checkRate(fee);
  return sourceCoins(wallet, source).reduce((n, u) => n + u.amount, 0);
}

function txActionRefusal(row: TxRow): TxActionRefusalName | null {
  if (row.amount >= 0 && row.category !== "sent") return "notSentByWallet";
  if (row.status.kind === "abandoned") return "alreadyAbandoned";
  if (row.txType === "generated") return "coinbase";
  if (row.status.confirmations > 0) return "confirmed";
  if (row.status.instantLocked) return "instantLocked";
  return null;
}

/** The wallet's row for `txid`, or `tx_not_found`. */
function rowOf(wallet: FixtureWallet, txid: string): TxRow {
  const row = wallet.rows.find((r) => r.txid === txid);
  if (!row) throw fail("tx_not_found", `transaction not found: ${txid}`);
  return row;
}

const refused = (refusal: TxActionRefusalName) => fail("tx_action_refused", `refused: ${refusal}`, { kind: "txAction", refusal });

// ---------------------------------------------------------------- the handlers

export interface SendDeps {
  wallet(w: World, walletId: string): FixtureWallet;
  /** An unused `spend` grant of the wallet that a key can use, or `null`. */
  spendGrant(w: World, grantId: string, walletId: string): FixtureGrant | null;
  /** Redeems (or revokes) a grant. */
  useGrant(w: World, grantId: string): void;
}

const unknownHandle = (h: number) => fail("app.unknown_handle", `unknown handle: ${h}`);

/** An address of the wallet's receiving chain: the same one for an index every run. */
function receiveAddress(wallet: FixtureWallet, index: number, label: string | null): AddressInfo {
  const seed = parseInt(wallet.summary.walletId.slice(0, 6), 16) + index * 7919;
  return {
    address: makeIds(prng(seed)).address(),
    chain: "receiving",
    index,
    derivationPath: `m/44'/1'/0'/0/${index}`,
    used: false,
    label,
    balance: 0,
    txCount: 0,
  };
}

/** Whether the wallet owns `address`: a coin's, or a receiving address it issued. */
export function owns(wallet: FixtureWallet, address: string): boolean {
  if (wallet.coins.some((u) => u.address === address)) return true;
  for (let i = 0; i <= wallet.receive.issued; i++) if (receiveAddress(wallet, i, null).address === address) return true;
  return false;
}

/** Issues the next receiving address, up to the gap limit. */
function issueAddress(wallet: FixtureWallet, label: string | null): AddressInfo {
  if (wallet.receive.issued - wallet.receive.firstUnused + 1 >= GAP_LIMIT) throw fail("gap_limit", "gap limit reached");
  wallet.receive.issued += 1;
  return receiveAddress(wallet, wallet.receive.issued, label);
}

function changed({ world, emit }: Ctx, wallet: FixtureWallet, txids: string[]) {
  const network = world.launch.network.dirName;
  emit({ kind: "balances", network, walletId: wallet.summary.walletId, balances: wallet.summary.balances });
  emit({ kind: "historyChanged", network, walletId: wallet.summary.walletId, txids });
}

function moveBalance(wallet: FixtureWallet, delta: number) {
  const b = wallet.summary.balances;
  if (!b) return;
  wallet.summary = { ...wallet.summary, balances: { ...b, confirmed: b.confirmed + delta, total: b.total + delta } };
}

export function sendHandlers({ wallet, spendGrant, useGrant }: SendDeps): Handlers {
  const draftOf = (w: World, h: number) => {
    const found = w.send.drafts[h];
    if (!found) throw unknownHandle(h);
    return found;
  };
  const preparedOf = (w: World, d: number, p: number) => {
    const found = w.send.prepared[p];
    if (!found || found.draft !== d) throw unknownHandle(p);
    return found;
  };
  /** Gives a prepared transaction's reserved inputs back and drops its handle. */
  const release = (w: World, p: number) => {
    const prep = w.send.prepared[p];
    if (!prep) return;
    for (const u of wallet(w, prep.walletId).coins)
      if (prep.summary.inputs.some((i) => sameOutpoint(i.outpoint, u.outpoint))) Object.assign(u, { reserved: false, spendable: true });
    delete w.send.prepared[p];
  };
  /** The wallet records a transaction it handed to the network (once): its inputs are spent, its
   * change is a new coin, a row and the balance move. */
  const handOver = (ctx: Ctx, prep: Prepared) => {
    const s = prep.summary;
    prep.handedOver = true;
    if (ctx.world.send.sent[s.txid]) return;
    const w = wallet(ctx.world, prep.walletId);
    const spent = w.coins.filter((u) => s.inputs.some((i) => sameOutpoint(i.outpoint, u.outpoint)));
    w.coins = w.coins.filter((u) => !spent.includes(u));
    const changeOut = s.outputs.findIndex((o) => o.isChange && o.isMine);
    const change: OutPoint | null = changeOut < 0 ? null : { txid: s.txid, vout: changeOut };
    if (change)
      w.coins.push({
        ...spent[0],
        outpoint: change,
        address: s.outputs[changeOut].address!,
        amount: s.outputs[changeOut].amount,
        confirmations: 0,
        blockHeight: null,
        timestamp: secs(FIXTURE_NOW),
        // The fixture's network InstantSend-locks it at once, so it can pay again.
        instantLocked: true,
        chainLocked: false,
        reserved: false,
        label: null,
        isChange: true,
        spendable: true,
      });
    const row: TxRow = {
      txid: s.txid,
      recordIndex: 0,
      txType: "sendToAddress",
      category: "sent",
      status: { kind: "unconfirmed", confirmations: 0, instantLocked: false, chainLocked: false, maturesIn: null },
      timestamp: secs(FIXTURE_NOW),
      blockHeight: null,
      amount: -s.totalDebit,
      fee: s.fee,
      address: prep.recipient.address,
      label: prep.recipient.label,
      countsTowardBalance: true,
      involvesWatchOnly: false,
    };
    w.rows = [row, ...w.rows];
    moveBalance(w, -s.totalDebit);
    ctx.world.send.sent[s.txid] = { walletId: prep.walletId, spent, change };
    changed(ctx, w, [s.txid]);
  };
  const set = (w: World, h: number, patch: Partial<Draft>) => {
    w.send.drafts[h] = { ...draftOf(w, h), ...patch };
    return null;
  };

  return {
    fee_policy: () => feePolicy(),
    classify_address: ({ network, text }) => classifyAddress(network, text),
    parse_amount: ({ text, unit }) => parseAmount(text, unit),
    build_payment_uri: ({ address, amount, label, message, user }) => paymentUri(address, amount, label, message, user),
    qr_matrix: ({ text }) => qrMatrix(text),
    payment_authentication: (_, { world }) => world.paymentAuth,

    utxos: ({ walletId, filter }, { world }) =>
      wallet(world, walletId).coins.filter(
        (u) =>
          (filter.includeLocked || !u.userLocked) &&
          (!filter.fullyMixedOnly || u.coinjoinRounds !== null) &&
          (filter.minConfirmations === null || u.confirmations >= filter.minConfirmations),
      ),
    coin_selection_summary: ({ walletId, outpoints, payAmounts, fee, allChangeToFee }, { world }) => {
      checkRate(fee);
      const coins = wallet(world, walletId).coins;
      const found = outpoints.map((o) => coins.find((u) => sameOutpoint(u.outpoint, o) && u.spendable));
      const usable = found.filter((u): u is Utxo => !!u);
      const amount = usable.reduce((n, u) => n + u.amount, 0);
      const pay = payAmounts.reduce((n, a) => n + a, 0);
      const bytes = sizeOf(usable.length, payAmounts.length + 1);
      let txFee = feeFor(bytes, rateOf(fee));
      let change = Math.max(0, amount - pay - txFee);
      const changeToFee = allChangeToFee || (change > 0 && change < DUST);
      if (changeToFee) {
        txFee += change;
        change = 0;
      }
      return {
        quantity: usable.length,
        amount,
        bytes,
        fee: txFee,
        afterFee: Math.max(0, amount - txFee),
        change,
        changeToFee,
        insufficientFunds: amount < pay + txFee,
        feeTolerancePerInput: feeFor(148, rateOf(fee)),
        unavailable: outpoints.filter((_, i) => !found[i]),
      };
    },
    max_spendable: ({ walletId, source, fee }, { world }) => maxSpendable(wallet(world, walletId), source, fee),

    new_tx_draft: ({ walletId }, { world }) => {
      wallet(world, walletId);
      const h = ++world.send.handles;
      world.send.drafts[h] = { walletId, recipients: [], source: { kind: "any" }, fee: { kind: "recommended", targetBlocks: 6 }, change: { kind: "auto" } };
      return h;
    },
    // A transaction that may be on the network keeps its inputs; its handle goes with the draft.
    close_draft: ({ draft }, { world }) => {
      if (!world.send.drafts[draft]) return false;
      for (const [p, prep] of Object.entries(world.send.prepared))
        if (prep.draft === draft) prep.handedOver ? delete world.send.prepared[Number(p)] : release(world, Number(p));
      delete world.send.drafts[draft];
      return true;
    },
    draft_set_recipients: ({ draft, recipients }, { world }) => set(world, draft, { recipients }),
    draft_set_source: ({ draft, source }, { world }) => set(world, draft, { source }),
    draft_set_fee: ({ draft, fee }, { world }) => set(world, draft, { fee }),
    draft_set_change: ({ draft, change }, { world }) => set(world, draft, { change }),
    draft_estimate: ({ draft }, { world }) => {
      const d = draftOf(world, draft);
      const { inputs: _, ...estimate } = build(world, wallet(world, d.walletId), d);
      return estimate;
    },
    // dash-qt "Create Unsigned" (QT-077): the draft's plan as a PSBT the PSBT tab can analyze,
    // sign and broadcast (tools.ts); nothing is signed or reserved.
    draft_create_unsigned: ({ draft }, { world }) => {
      const d = draftOf(world, draft);
      const w = wallet(world, d.walletId);
      const built = build(world, w, d);
      const change = d.change.kind === "address" ? d.change.address : receiveAddress(w, 2000 + draft, null).address;
      const psbt = btoa(`psbt\xffdraft-${draft}`);
      world.tools.psbts[psbt] = {
        outputs: [
          ...d.recipients.map((r, i) => ({ address: r.address, amount: i === 0 ? built.totalSent : r.amount, isMine: owns(w, r.address) })),
          ...(built.change === null ? [] : [{ address: change, amount: built.change, isMine: d.change.kind === "auto" || owns(w, change) }]),
        ],
        fee: built.fee,
        inputs: built.inputs.length,
        signedInputs: 0,
        signer: d.walletId,
      };
      return psbt;
    },

    // The `spend` grant caps what leaves the wallet: the outputs it does not own (recipients, a
    // foreign change address) plus the fee. The plan is checked before the grant, as the engine.
    draft_prepare: ({ draft, grantId }, { world }) => {
      const d = draftOf(world, draft);
      const w = wallet(world, d.walletId);
      const built = build(world, w, d);
      const grant = spendGrant(world, grantId, d.walletId);
      if (!grant) throw fail("send.grant_invalid", "grant invalid");
      const h = world.send.handles + 1;
      const recipient = d.recipients[0];
      const changeAddress = d.change.kind === "address" ? d.change.address : receiveAddress(w, 1000 + h, null).address;
      const outputs = [
        ...d.recipients.map((r) => ({ address: r.address, amount: r === recipient ? built.totalSent : r.amount, isChange: false, label: r.label, isMine: owns(w, r.address) })),
        ...(built.change === null ? [] : [{ address: changeAddress, amount: built.change, isChange: true, label: null, isMine: d.change.kind === "auto" || owns(w, changeAddress) }]),
      ];
      const externalSent = outputs.filter((o) => !o.isMine).reduce((n, o) => n + o.amount, 0);
      const debit = externalSent + built.fee;
      const cap = grant.grant.purpose.kind === "spend" ? grant.grant.purpose.maxDuffs : 0;
      useGrant(world, grantId);
      if (debit > cap) throw fail("send.grant_exceeded", "grant exceeded", { kind: "spendCap", maxDuffs: cap, outflow: debit });
      for (const u of built.inputs) Object.assign(u, { reserved: true, spendable: false });
      world.send.handles = h;
      const summary: PreparedTxSummary = {
        txid: `5e17${h.toString(16).padStart(4, "0")}`.repeat(8),
        fee: built.fee,
        feeRatePerKb: rateOf(d.fee),
        sizeBytes: built.sizeBytes,
        inputs: built.inputs.map((u) => ({ outpoint: u.outpoint, address: u.address, amount: u.amount })),
        outputs,
        totalSent: built.totalSent,
        totalDebit: debit,
        externalSent,
      };
      world.send.prepared[h] = { draft, walletId: d.walletId, summary, recipient, handedOver: false };
      return { handle: h, summary };
    },

    // E0-13 D8: accepted or released, the handle is dropped; after `broadcast_unknown` it stays.
    // Only a first attempt is released: once unknown, a failure stays unknown. The wallet records
    // the transaction when it is handed over, whatever the outcome.
    draft_broadcast: ({ draft, prepared }, ctx) => {
      const { world } = ctx;
      const prep = preparedOf(world, draft, prepared);
      const mode = world.send.broadcast;
      if (mode === "noPeers" || mode === "rejected") {
        if (prep.handedOver) throw fail("send.broadcast_unknown", "no answer from the network");
        release(world, prepared);
        throw mode === "noPeers"
          ? fail("send.no_peers", "no peers")
          : fail("send.broadcast_rejected", "rejected", { kind: "reason", reason: "bad-txns-inputs-missingorspent" });
      }
      handOver(ctx, prep);
      if (mode === "unknown") throw fail("send.broadcast_unknown", "no answer from the network");
      delete world.send.prepared[prepared];
      return { txid: prep.summary.txid };
    },
    draft_abandon: ({ draft, prepared }, { world }) => {
      if (preparedOf(world, draft, prepared).handedOver) throw fail("send.prepared_tx_spent", "the transaction may be on the network");
      release(world, prepared);
      return null;
    },

    // The transaction actions (QT-105) and the detail sheet's extras.
    tx_detail_extras: ({ walletId, txid }, { world }) => {
      const row = rowOf(wallet(world, walletId), txid);
      const d = txDetails(row);
      const refusal = txActionRefusal(row);
      const extras: TxDetailExtrasInfo = {
        txid,
        isCoinbase: row.txType === "generated",
        totalCredit: d.outputs.filter((o) => o.isMine).reduce((n, o) => n + (o.amount ?? 0), 0),
        totalDebit: d.inputs.filter((i) => i.isMine).reduce((n, i) => n + (i.amount ?? 0), 0),
        net: row.amount,
        maturesIn: row.status.maturesIn,
        inMempool: null,
        abandoned: row.status.kind === "abandoned",
        canAbandon: refusal === null,
        canResend: refusal === null,
        dustLockedOutputs: [],
        lastAnnouncedAt: world.send.announced[txid] ?? null,
      };
      return extras;
    },
    abandon_transaction: ({ walletId, txid }, ctx) => {
      const w = wallet(ctx.world, walletId);
      const row = rowOf(w, txid);
      const refusal = txActionRefusal(row);
      if (refusal) throw refused(refusal);
      w.rows = w.rows.map((r) => (r.txid === txid ? { ...r, status: { ...r.status, kind: "abandoned" }, countsTowardBalance: false } : r));
      const mine = ctx.world.send.sent[txid];
      if (mine) {
        w.coins = [...w.coins.filter((u) => !(mine.change && sameOutpoint(u.outpoint, mine.change))), ...mine.spent.map((u) => ({ ...u, reserved: false, spendable: true }))];
        delete ctx.world.send.sent[txid];
      }
      moveBalance(w, -row.amount);
      changed(ctx, w, [txid]);
      return null;
    },
    resend_transaction: ({ walletId, txid }, { world }) => {
      const refusal = txActionRefusal(rowOf(wallet(world, walletId), txid));
      if (refusal) throw refused(refusal);
      world.send.announced[txid] = secs(FIXTURE_NOW);
      return null;
    },
    set_tx_label: ({ walletId, txid, label }, ctx) => {
      const w = wallet(ctx.world, walletId);
      rowOf(w, txid);
      const text = label?.trim() || null;
      w.rows = w.rows.map((r) => (r.txid === txid ? { ...r, label: text } : r));
      ctx.emit({ kind: "historyChanged", network: ctx.world.launch.network.dirName, walletId, txids: [txid] });
      return null;
    },

    // Receive: the first unused receiving address, fresh ones up to the gap limit, requests.
    current_receive_address: ({ walletId }, { world }) => {
      const w = wallet(world, walletId);
      return receiveAddress(w, w.receive.firstUnused, null);
    },
    next_receive_address: ({ walletId, label }, { world }) => issueAddress(wallet(world, walletId), label),
    create_receive_request: ({ walletId, amount, label, message }, { world }) => {
      const w = wallet(world, walletId);
      const { address } = issueAddress(w, label);
      const request: ReceiveRequest = {
        id: w.receive.nextRequest++,
        createdAt: secs(FIXTURE_NOW),
        address,
        amount,
        label,
        message,
        username: null,
        uri: "",
      };
      // Before it is stored: an amount out of range stores nothing, as the engine.
      const shown = withUsername(request, mainUsername(world.dashpay));
      w.receive.requests = [request, ...w.receive.requests];
      return shown;
    },
    // The username is the current main name, not stored (DP3-03 D18).
    receive_requests: ({ walletId }, { world }) =>
      wallet(world, walletId).receive.requests.map((r) => withUsername(r, mainUsername(world.dashpay))),
    receive_username: ({ walletId }, { world }) => {
      wallet(world, walletId);
      return mainUsername(world.dashpay);
    },
    delete_receive_request: ({ walletId, requestId }, { world }) => {
      const w = wallet(world, walletId);
      if (!w.receive.requests.some((r) => r.id === requestId)) throw fail("request_not_found", `no request ${requestId}`);
      w.receive.requests = w.receive.requests.filter((r) => r.id !== requestId);
      return null;
    },
  };
}
