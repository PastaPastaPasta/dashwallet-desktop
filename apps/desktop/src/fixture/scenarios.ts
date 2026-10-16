// dw-fixture-module: never in a production build (vite.config.ts, noFixtureModules).
// Named fixture scenarios. Each builds a fresh `World`: the state the fixture's command
// handlers read and change. Pick one with `?scenario=<name>` (dev server, Playwright) or
// `installFixture({ scenario })` (vitest).

import type { AuthGrant } from "../api/generated/AuthGrant";
import type { DashPayStatus } from "../api/generated/DashPayStatus";
import type { DispatchStateName } from "../api/generated/DispatchStateName";
import type { FlowLockReport } from "../api/generated/FlowLockReport";
import type { FlowOutcomeName } from "../api/generated/FlowOutcomeName";
import type { LaunchInfo } from "../api/generated/LaunchInfo";
import type { LinkRoute } from "../api/generated/LinkRoute";
import type { Lease } from "../api/generated/Lease";
import type { LockOutcome } from "../api/generated/LockOutcome";
import type { LockStatus } from "../api/generated/LockStatus";
import type { RecentNotice } from "../api/generated/RecentNotice";
import type { ShellSync } from "../api/generated/ShellSync";
import type { SyncStatus } from "../api/generated/SyncStatus";
import type { ThemeName } from "../api/generated/ThemeName";
import type { TxRow } from "../api/generated/TxRow";
import type { WalletSummary } from "../api/generated/WalletSummary";
import { IDENTITY, noIdentity, registered, registering, type DashPayWorld } from "./dashpay";
import type { Utxo } from "../api/generated/Utxo";
import { buildCoins, newReceiveWorld, newSendWorld, type ReceiveWorld, type SendWorld } from "./send";
import { newToolsWorld, type ToolsWorld } from "./tools";
import { newSettingsWorld, withAutomaticBackups, type SettingsWorld } from "./settings";
import { scanWorld, type ScanWorld } from "./scan";
import { MAIN_PHRASE, SECOND_PHRASE, throttleWaitSecs } from "./vault";
import { buildRows, caughtUp, FUNDED_BALANCES, makeIds, prng, XSS_PAYLOADS, ZERO_BALANCES, type HistoryVariant } from "./wallet";

export const FIXTURE_PASSPHRASE = "demo";
export const FIXTURE_WALLET_ID = "a1".repeat(32);
/** The `two-wallets` scenario's second wallet. */
export const SECOND_WALLET_ID = "b2".repeat(32);

export interface FixtureWallet {
  summary: WalletSummary;
  rows: TxRow[];
  coins: Utxo[];
  receive: ReceiveWorld;
}

/** What the fixture's engine knows. Handlers mutate it, like the engine's own state. */
export interface World {
  launch: LaunchInfo;
  passphrase: string;
  lock: LockStatus;
  /** The recovery phrase of each wallet whose secrets the vault holds (U-02). */
  phrases: Record<string, string>;
  wallets: FixtureWallet[];
  sync: SyncStatus;
  /** The first wallet's DashPay state. */
  dashpay: DashPayWorld;
  /** Another wallet's own DashPay state, by wallet id; a wallet not here shares `dashpay`. */
  dashpayOf?: Record<string, DashPayWorld>;
  leases: Lease[];
  /** The vault grants `authorize` issued since the last lock or unlock (each revokes them all). */
  grants: FixtureGrant[];
  /** `begin_flow`'s lease ids (`lease-` and 32 hex digits) to their `Lease.id` in `leases`. */
  flowLeases: Record<string, string>;
  /** Grant and lease ids issued so far: they are drawn from it, so runs repeat. */
  issued: number;
  /** `dispatch_status` by artifact. */
  dispatch: Record<string, DispatchStateName>;
  lastLockReport: LockOutcome | null;
  /** What the next `lock` cuts and reports (E0-13 D3's bounded report). */
  cut: Pick<LockOutcome, "flows" | "flowsOmitted">;
  notices: RecentNotice[];
  /** Deep links waiting for `take_links` (`FixtureHandle.deliverLinks`), already routed. */
  links: LinkRoute[];
  /** The last `shell_sync`: what the native menus would show. */
  shellSync: ShellSync | null;
  /** "Require authentication for every payment" (E0-13 D11; on by default). */
  paymentAuth: boolean;
  /** The vault's wait after a wrong passphrase (`retryAfterSecs`); null: none. */
  throttleSecs: number | null;
  /** When that wait ends (ms since epoch), for `lock_status`; null: no wait. */
  throttledUntil: number | null;
  /** Drafts, prepared transactions and what the next broadcast does (send.ts). */
  send: SendWorld;
  /** Tools: node facts, console, peers, repair, address book, messages, PSBT, files (tools.ts). */
  tools: ToolsWorld;
  /** The settings file, load states, backups and files (settings.ts, U-04). */
  settings: SettingsWorld;
  /** The QR scan commands' screen picker (scan.ts, DP2-08). */
  scan: ScanWorld;
}

/** A vault grant: single use (`used` once redeemed or revoked); `ownKey` when it was issued
 * while the vault held no data key, so it carries its own. */
export interface FixtureGrant {
  grant: AuthGrant;
  used: boolean;
  ownKey: boolean;
}

/** dw-app's `MAX_REPORT_WALLETS`: a lock report lists this many wallets and counts the rest. */
export const MAX_REPORT_WALLETS = 16;

const unlocked = (walletIds: string[]): LockStatus => ({
  state: "unlocked",
  encrypted: true,
  quickUnlockEnrolled: false,
  failedAttempts: 0,
  retryAfterSecs: null,
  walletsWithSecrets: walletIds,
});

function wallet(variant: HistoryVariant): FixtureWallet {
  return {
    summary: {
      walletId: FIXTURE_WALLET_ID,
      name: variant === "xss" ? XSS_PAYLOADS.walletName : "Main",
      watchOnly: false,
      hasMnemonic: true,
      hd: true,
      birthHeight: 1_200_000,
      createdAt: null,
      balances: variant === "empty" ? ZERO_BALANCES : FUNDED_BALANCES,
    },
    rows: buildRows(variant),
    coins: buildCoins(variant),
    receive: newReceiveWorld(),
  };
}

/** A wallet `import_wallet` adds: no history yet, and its birth height the tip's. */
export function emptyWallet(walletId: string, name: string): FixtureWallet {
  const w = wallet("empty");
  w.summary = { ...w.summary, walletId, name };
  return w;
}

/** The funded wallet behind the lock screen after `failedAttempts` wrong passphrases, with the
 * wait dw-vault reports for them (the fixture's clock stands still, so it never runs out). */
function lockedAfter(w: World, failedAttempts: number): World {
  w.lock = { ...w.lock, state: "locked", failedAttempts, retryAfterSecs: throttleWaitSecs(failedAttempts) || null };
  return w;
}

function base(theme: ThemeName | null, variant: HistoryVariant = "default"): World {
  const main = wallet(variant);
  return {
    launch: {
      selftest: false,
      selftestPauseMs: 0,
      selftestUnlock: false,
      selftestLinkTrace: false,
      theme,
      // Testnet, like the reference images.
      network: { kind: "testnet", dirName: "testnet", unitName: "tDASH" },
      appVersion: "0.1.0",
      engineVersion: "fixture",
    },
    passphrase: FIXTURE_PASSPHRASE,
    lock: unlocked([FIXTURE_WALLET_ID]),
    phrases: { [FIXTURE_WALLET_ID]: MAIN_PHRASE },
    wallets: [main],
    sync: caughtUp(),
    dashpay: noIdentity(),
    leases: [],
    grants: [],
    flowLeases: {},
    issued: 0,
    dispatch: {},
    lastLockReport: null,
    cut: { flows: [], flowsOmitted: 0 },
    notices: [],
    links: [],
    shellSync: null,
    paymentAuth: true,
    throttleSecs: null,
    throttledUntil: null,
    send: newSendWorld(),
    tools: newToolsWorld(main),
    settings: withAutomaticBackups(newSettingsWorld(), FIXTURE_WALLET_ID, MAIN_PHRASE, FIXTURE_PASSPHRASE),
    scan: scanWorld(),
  };
}

const lease = (id: string, flow: Lease["flow"], fundsCommitted: boolean, inFlight = 0): Lease => ({
  id,
  walletId: FIXTURE_WALLET_ID,
  flow,
  state: { kind: "active" },
  ownKey: true,
  keyExpiresInSecs: 240,
  fundsCommitted,
  budgets: [{ purpose: "funding", ceiling: "150000000", spent: fundsCommitted ? "120000000" : "0" }],
  inFlight,
  callRunning: inFlight > 0,
});

/** The engine's outcome order, weakest first (E0-04 §8.4): a flow's is the strongest of its
 * artifacts'. And dw-app's listing order, unresolved first (`keep_unresolved_first`). */
const STRENGTH: FlowOutcomeName[] = ["cancelled", "maybeSent", "willBeSent", "sent"];
const LISTING: FlowOutcomeName[] = ["maybeSent", "willBeSent", "sent", "cancelled"];
const listed = <T extends { outcome: FlowOutcomeName }>(items: T[]) => items.toSorted((a, b) => LISTING.indexOf(a.outcome) - LISTING.indexOf(b.outcome));

/** A 32-byte artifact id (a txid or a state-transition hash): plain lower-case hex. */
const artifactId = (seed: string) => seed.repeat(64 / seed.length);
/** A step id, as a running call or a funding step is named. */
const step = (flow: string, draft: string, part: string) => `${flow}/${draft}/${part}`;

/** A cut flow as the bridge reports it: the facade's lease id (`lease-` and 32 hex digits, the
 * first 8 being `Lease.id`), the outcome derived from the artifacts, those listed unresolved first. */
const flow = (lease: string, kind: FlowLockReport["flow"], artifacts: [string, FlowOutcomeName][], omitted = 0): FlowLockReport => ({
  lease: `lease-${lease.repeat(4)}`,
  walletId: FIXTURE_WALLET_ID,
  flow: kind,
  outcome: artifacts.reduce<FlowOutcomeName>((o, [, a]) => (STRENGTH.indexOf(a) > STRENGTH.indexOf(o) ? a : o), "cancelled"),
  artifacts: listed(artifacts.map(([artifact, outcome]) => ({ artifact, outcome }))),
  artifactsOmitted: omitted,
});

/** Locks `w` with these flows cut, listed as the bridge lists them; the lock revokes each flow's
 * lease. The dispatch state of every listed artifact starts at its outcome. With `walletsOmitted`
 * above 0 the vault holds that many wallets with secrets beyond the 16 a report lists. */
function cutting(w: World, flows: FlowLockReport[], flowsOmitted = 0, walletsOmitted = 0): World {
  if (walletsOmitted > 0) {
    const others = Array.from({ length: MAX_REPORT_WALLETS - 1 + walletsOmitted }, (_, i) => hex(i + 1).repeat(32));
    w.lock = { ...w.lock, walletsWithSecrets: [...w.lock.walletsWithSecrets, ...others].sort() };
  }
  w.dashpay = registered();
  w.leases = flows.map((f) => lease(f.lease.slice("lease-".length, "lease-".length + 8), f.flow, f.flow === "registration" || f.flow === "topUp" || f.flow === "acceptAndPay"));
  w.dispatch = Object.fromEntries(flows.flatMap((f) => f.artifacts.map((a) => [a.artifact, a.outcome === "cancelled" ? "notSent" : a.outcome])));
  w.cut = { flows: listed(flows), flowsOmitted };
  return w;
}

const hex = (i: number) => i.toString(16).padStart(2, "0");
/** A contact request cut by the lock: its state transition, with this outcome. */
const request = (i: number, outcome: FlowOutcomeName) =>
  flow(`5eed00${hex(i)}`, "contactRequest", outcome === "cancelled" ? [] : [[artifactId(`5eed00${hex(i)}`), outcome]]);

/** A registration whose funding was sent and whose identity call may have gone out: the engine
 * lists the funding step too, so it reads `sent` (E0-04 §8.4). */
const registrationCut = () =>
  flow("3f9a1c27", "registration", [
    [step("registration", "3f9a1c27", "funding"), "sent"],
    [step("registration", "3f9a1c27", "identity"), "maybeSent"],
  ]);

/** The cut flows of a full report (E0-13 D3: 16 flows of 8 artifacts): a registration whose
 * identity call was still running, a top-up with these artifact outcomes, and 14 requests. */
function sixteen(requests: FlowOutcomeName, topUp: FlowOutcomeName[], topUpOmitted: number): FlowLockReport[] {
  return [
    registrationCut(),
    flow("8be04d11", "topUp", topUp.map((o, i) => [artifactId(`8be04d${hex(i)}`), o]), topUpOmitted),
    ...Array.from({ length: 14 }, (_, i) => request(i, requests)),
  ];
}

/** Every listed flow and artifact is pending and more were cut: the omitted ones may be pending
 * too, and no query names them. Unresolved for good. */
const omittedPending = (w: World) => cutting(w, sixteen("willBeSent", Array(8).fill("willBeSent"), 7), 3, 2);

/** Counts above 0, but the order proves nothing omitted is pending: the listed flows end with
 * cancelled ones and the top-up's listed artifacts with sent ones (the top-up itself reads
 * `sent`, its strongest). Unresolved until the listed pending artifacts settle and the
 * registration's call, still running, ends. */
function omittedSettled(w: World): World {
  cutting(w, sixteen("cancelled", ["willBeSent", ...Array<FlowOutcomeName>(7).fill("sent")], 4), 2, 2);
  w.leases[0] = { ...w.leases[0], inFlight: 1, callRunning: true };
  return w;
}

/** Flows in flight when the user locks, all listed: a registration whose funding was sent while
 * its identity call may have gone out (E0-04 §8.4: it reads `sent`, yet something of it is
 * pending), a top-up handed off, a sent payment and a cancelled request. */
const listedFlows = (w: World) =>
  cutting(w, [
    registrationCut(),
    flow("8be04d11", "topUp", [[artifactId("8be04d11"), "willBeSent"]]),
    flow("c0ffee01", "acceptAndPay", [[artifactId("c0ffee01"), "sent"]]),
    request(0, "cancelled"),
  ]);

/** An accept-and-pay whose payment was sent while its `accept_request` call may have gone out.
 * A call is reported under its label, which `dispatchStatus` cannot name: unresolved for good. */
const unnamedCall = (w: World) =>
  cutting(w, [
    flow("c0ffee01", "acceptAndPay", [
      [artifactId("c0ffee01"), "sent"],
      ["accept_request", "maybeSent"],
    ]),
  ]);

/** Everything resolved: the payment was sent and the request cancelled. */
function resolvedFlows(w: World): World {
  return cutting(w, [flow("c0ffee01", "acceptAndPay", [[artifactId("c0ffee01"), "sent"]]), request(0, "cancelled")]);
}

/** The engine's counterparty attribution (DP3-02) for the first unlabelled payment from Bob
 * (named "Bob Builder") and to Dave (no display name: `@dave`). */
function attributePayments(w: World) {
  const party = (name: "bob" | "dave") => {
    const c = w.dashpay.contacts.find((x) => x.summary.contact === IDENTITY[name])!.summary;
    return { identity: c.contact, username: c.username, displayName: c.displayName, avatarUrl: null };
  };
  const rows = w.wallets[0].rows;
  const first = (type: TxRow["txType"]) => rows.find((r) => r.txType === type && !r.label && r.status.kind === "confirmed");
  const attribute = (row: TxRow, name: "bob" | "dave", direction: "sent" | "received") => {
    row.counterparty = party(name);
    // The contact's activity lists it too (DP2-09), so its row opens the transaction.
    const items = (w.dashpay.activity[IDENTITY[name]] ??= []);
    items.push({ txid: row.txid, direction, amount: Math.abs(row.amount) - (direction === "sent" ? (row.fee ?? 0) : 0), height: row.blockHeight, timestamp: row.timestamp });
  };
  attribute(first("recvWithAddress")!, "bob", "received");
  attribute(first("sendToAddress")!, "dave", "sent");
}

/** DP3-07 D10: besides Bob's and Dave's, the newest payment from Marta (no relation to the main
 * identity), "Coffee refund" from Bob (a label over the contact), an older payment whose
 * counterparty's names have not landed yet, and the oldest payment, to Judy (hidden). */
function attributeMore(w: World) {
  attributePayments(w);
  const rows = w.wallets[0].rows;
  rows[0].counterparty = { identity: IDENTITY.marta, username: "marta", displayName: "Marta", avatarUrl: null };
  // The newest labelled receipt, "Coffee refund" (its text stays wallet.ts's sentinel).
  const refund = rows.find((r) => r.label && r.amount > 0)!;
  refund.counterparty = { identity: IDENTITY.bob, username: "bob", displayName: "Bob Builder", avatarUrl: null };
  w.dashpay.activity[IDENTITY.bob].unshift({ txid: refund.txid, direction: "received", amount: refund.amount, height: refund.blockHeight, timestamp: refund.timestamp });
  const nameless = rows.find((r) => r.txType === "recvWithAddress" && r.category === "received" && !r.label && !r.counterparty && r.status.kind === "confirmed")!;
  nameless.counterparty = { identity: IDENTITY.mateo, username: null, displayName: null, avatarUrl: null };
  // The oldest plain payment went to Judy, a contact the user hid: listed, in the collapsed Hidden section.
  const hidden = rows.findLast((r) => r.txType === "sendToAddress" && !r.label && !r.counterparty && r.status.kind === "confirmed")!;
  hidden.counterparty = { identity: IDENTITY.judy, username: "judy", displayName: "Judy", avatarUrl: null };
  (w.dashpay.activity[IDENTITY.judy] ??= []).push({ txid: hidden.txid, direction: "sent", amount: Math.abs(hidden.amount) - (hidden.fee ?? 0), height: hidden.blockHeight, timestamp: hidden.timestamp });
}

function twoWallets(w: World): World {
  w.wallets.push(emptyWallet(SECOND_WALLET_ID, "Savings"));
  w.lock = unlocked([FIXTURE_WALLET_ID, SECOND_WALLET_ID]);
  w.phrases[SECOND_WALLET_ID] = SECOND_PHRASE;
  return w;
}

/** Three mixed coins (100.001, 10.0001 and 1.00001 DASH denominations), counted in the balances. */
function withCoinJoinCoins(w: World): World {
  const main = w.wallets[0];
  const { hex } = makeIds(prng(0xc01d));
  const mixed = [10_000_100_000, 1_000_010_000, 100_001_000].map((amount, i): Utxo => ({
    ...main.coins[0],
    outpoint: { txid: hex(64), vout: i },
    amount,
    label: null,
    coinjoinDenominated: true,
    coinjoinRounds: 4 + i,
  }));
  main.coins = [...main.coins, ...mixed];
  const sum = mixed.reduce((n, u) => n + u.amount, 0);
  const b = main.summary.balances!;
  main.summary = { ...main.summary, balances: { ...b, confirmed: b.confirmed + sum, total: b.total + sum, coinjoin: b.coinjoin + sum } };
  return w;
}

/** "alice" contested: her identity holds the temporary "alice007" while masternodes vote. */
function contestPending(w: World, endsAt: number | null): World {
  const world = registered();
  w.dashpay = {
    ...world,
    status: { kind: "contestPending", identity: IDENTITY.alice, label: "alice", endsAt },
    identities: world.identities.map((i) => ({ ...i, names: ["alice007"], mainName: "alice007" })),
  };
  return w;
}

/** The wallet's DashPay status set to `status`, no identity on file (DP1-08). */
function withStatus(w: World, status: DashPayStatus): World {
  w.dashpay = { ...noIdentity(), status };
  return w;
}

/** Syncing: filters 60% through, the masternode list not started (65% in all). */
function syncing(): SyncStatus {
  const s = caughtUp();
  return {
    ...s,
    phases: s.phases.map((p) => {
      if (p.phase === "filters") return { ...p, currentHeight: Math.round(p.targetHeight! * 0.6), done: false };
      if (p.phase === "masternodes") return { ...p, currentHeight: 0, done: false };
      return p;
    }),
    activePhase: "filters",
    caughtUp: false,
  };
}

const SCENARIOS = {
  /** The reference wallet: balance, a month of history, caught up, unlocked. */
  funded: (t) => base(t),
  /** A new wallet: no history, zero balance. */
  empty: (t) => base(t, "empty"),
  /** No wallet on the network yet, and no vault. */
  "no-wallet": (t) => {
    const w = base(t);
    w.wallets = [];
    w.phrases = {};
    w.lock = { ...unlocked([]), state: "noVault", encrypted: false };
    return w;
  },
  /** A vault (passphrase `demo`) that holds no wallet yet: onboarding without a passphrase step. */
  "no-wallet-vault": (t) => {
    const w = base(t);
    w.wallets = [];
    w.phrases = {};
    w.lock = unlocked([]);
    return w;
  },
  /** The funded wallet in a vault without a passphrase: Settings ▸ Security offers Encrypt (QT-111). */
  unencrypted: (t) => {
    const w = base(t);
    w.lock = { ...w.lock, state: "unencrypted", encrypted: false };
    return w;
  },
  /** The funded wallet behind the lock screen (passphrase `demo`). */
  locked: (t) => {
    const w = base(t);
    w.lock = { ...w.lock, state: "locked" };
    return w;
  },
  /** Three wrong passphrases: the lock screen waits a minute (IOS-012). */
  "locked-throttled": (t) => lockedAfter(base(t), 3),
  /** Eight wrong passphrases: only Forgot passphrase is left (IOS-012). */
  "locked-disabled": (t) => lockedAfter(base(t), 8),
  /** Two wallets: the toolbar's wallet picker. */
  "two-wallets": (t) => twoWallets(base(t)),
  /** Two wallets behind the lock screen: Forgot passphrase asks which one (IOS-014). */
  "locked-two-wallets": (t) => lockedAfter(twoWallets(base(t)), 0),
  /** 10,003 transactions: paging and render cost. */
  large: (t) => base(t, "large"),
  /** Hostile labels and names (U6): they must render as text. */
  xss: (t) => base(t, "xss"),
  "dashpay-no-identity": (t) => base(t),
  /** A username registration funded and waiting for its InstantSend lock. */
  "dashpay-registering": (t) => {
    const w = base(t);
    w.dashpay = registering();
    w.leases = [lease("9a0b1c2d", "registration", true)];
    return w;
  },
  /** "alice", with contacts in every relation and channel state and two requests waiting;
   * payments from Bob and to Dave attributed in the history (DP3-02). */
  "dashpay-registered": (t) => {
    const w = base(t);
    w.dashpay = registered();
    attributePayments(w);
    return w;
  },
  /** DP3-07: `dashpay-registered` with more attributed payments among the newest rows (D10). */
  "dashpay-history-contacts": (t) => {
    const w = base(t);
    w.dashpay = registered();
    attributeMore(w);
    return w;
  },
  /** "alice" contested: her identity holds the temporary "alice007" while masternodes vote. The
   * chip says "@alice · Pending"; Contacts, the bell and the badge stay (BL-153 D11). */
  "dashpay-contest-pending": (t) => contestPending(base(t), 1_900_000_000),
  // DP1-08: the Home card's other banner states (the status read failing is `fail=dashpay_status:none`).
  /** No identity while the wallet syncs: Join DashPay disabled, "Available after sync finishes". */
  "dashpay-waiting-for-sync": (t) => {
    const w = withStatus(base(t), { kind: "noIdentity", reason: "waitingForSync" });
    w.sync = syncing();
    return w;
  },
  /** A watch-only wallet: it cannot join, and the chip is hidden (DP1-08 D3). */
  "dashpay-watch-only": (t) => {
    const w = withStatus(base(t), { kind: "noIdentity", reason: "watchOnly" });
    w.wallets[0].summary = { ...w.wallets[0].summary, watchOnly: true };
    return w;
  },
  /** DashPay's bring-up is looking for the wallet's identity. */
  "dashpay-starting": (t) => withStatus(base(t), { kind: "startupIncomplete", startup: "starting" }),
  /** No bring-up ran (a launch without sync). */
  "dashpay-not-started": (t) => withStatus(base(t), { kind: "startupIncomplete", startup: "notRun" }),
  /** The bring-up ran locked; unlocked for mixing only, the card offers the full unlock. */
  "dashpay-identity-unsettled": (t) => {
    const w = withStatus(base(t), { kind: "startupIncomplete", startup: "identityUnsettled" });
    w.lock = { ...w.lock, state: "unlockedMixingOnly" };
    return w;
  },
  /** The bring-up could not finish its discovery. */
  "dashpay-look-failed": (t) => withStatus(base(t), { kind: "startupIncomplete", startup: "discoveryFailed" }),
  /** A registration whose row is not listed (yet): the card's generic line. */
  "dashpay-registering-unread": (t) => {
    const w = base(t);
    w.dashpay = { ...registering(), registrations: [] };
    return w;
  },
  /** The registration waits for the full unlock (unlocked for mixing only). */
  "dashpay-registering-unlock": (t) => {
    const w = base(t);
    w.dashpay = registering();
    w.dashpay.registrations[0] = { ...w.dashpay.registrations[0], phase: "draft", waiting: "unlock", txid: null, holdsKey: false, fundsCommitted: false };
    w.lock = { ...w.lock, state: "unlockedMixingOnly" };
    return w;
  },
  /** The registration stopped, retryably: not enough Dash for its lock. */
  "dashpay-registering-paused": (t) => {
    const w = base(t);
    w.dashpay = registering();
    w.dashpay.registrations[0] = {
      ...w.dashpay.registrations[0],
      phase: "failed",
      waiting: null,
      txid: null,
      holdsKey: false,
      fundsCommitted: false,
      failure: { phase: "keysPrepared", code: "registration.funding_insufficient", retryable: true, needed: "3002260", available: "1000000" },
    };
    return w;
  },
  /** "alice" contested, the vote's end not read yet. */
  "dashpay-contest-pending-no-end": (t) => contestPending(base(t), null),
  /** Alice without a username (its name step failed): Finish username registration; My QR has no link (DP2-08 D12). */
  "dashpay-no-username": (t) => {
    const w = base(t);
    const world = registered();
    w.dashpay = { ...world, identities: world.identities.map((i) => ({ ...i, names: [], mainName: null })) };
    return w;
  },
  /** A sync pass holds Alice's names (S18): no card, the chip says "Updating…". */
  "dashpay-names-updating": (t) => {
    const w = base(t);
    const world = registered();
    w.dashpay = { ...world, identities: world.identities.map((i) => ({ ...i, names: [], mainName: null, namesUpdating: true })) };
    return w;
  },
  /** A restore found Alice; the engine's names pass that fetches her names has not ended (D23):
   * no card, the chip says "Updating…". */
  "dashpay-names-pending": (t) => {
    const w = base(t);
    const world = registered();
    w.dashpay = { ...world, identities: world.identities.map((i) => ({ ...i, names: [], mainName: null, namesCheck: "pending" as const })) };
    return w;
  },
  /** That names pass failed (D23): Alice may own a name, so no card and a neutral chip. */
  "dashpay-names-unknown": (t) => {
    const w = base(t);
    const world = registered();
    w.dashpay = { ...world, identities: world.identities.map((i) => ({ ...i, names: [], mainName: null, namesCheck: "failed" as const })) };
    return w;
  },
  /** Tools ▸ Repair: the registration's funding broadcast's outcome is unknown (E0-04.P2b). */
  "tools-stuck-funding": (t) => {
    const w = base(t);
    w.dashpay = registering();
    w.dashpay.registrations[0].waiting = "fundingUnknown";
    return w;
  },
  /** Tools ▸ Information: every node warning; the banner shows the most severe (QT-040). */
  "tools-warnings": (t) => {
    const w = base(t);
    w.tools.warnings = [
      { code: "prereleaseBuild", detail: "" },
      { code: "syncStalled", detail: "" },
      { code: "uncleanShutdown", detail: "" },
    ];
    return w;
  },
  /** The funded wallet watch-only: Pay's confirm sheet offers Create Unsigned only (U-05 D18). */
  "watch-only": (t) => {
    const w = base(t);
    w.wallets[0].summary = { ...w.wallets[0].summary, watchOnly: true };
    return w;
  },
  /** Coin Selection: CoinJoin denominations beside the plain coins, hidden by default (QT-071). */
  "tools-coinjoin": (t) => withCoinJoinCoins(base(t)),
  /** "alice" before her first contact: the Contacts empty state (DP2-07). */
  "dashpay-contacts-empty": (t) => {
    const w = base(t);
    w.dashpay = { ...registered(), contacts: [], requestTimes: {}, journal: [] };
    return w;
  },
  /** A sync pass holds the wallet's state before any contacts snapshot: "Updating contacts…". */
  "dashpay-contacts-updating": (t) => {
    const w = base(t);
    w.dashpay = { ...registered(), contactsUpdating: "noSnapshot" };
    return w;
  },
  /** A sync pass holds the wallet's state after a snapshot: the rows, under "Updating contacts…". */
  "dashpay-contacts-refreshing": (t) => {
    const w = base(t);
    w.dashpay = { ...registered(), contactsUpdating: "snapshot" };
    return w;
  },
  /** Unlocked for mixing only, Carol's channel still setting up: "Unlock to finish setting up". */
  "dashpay-contacts-mixing-only": (t) => {
    const w = base(t);
    w.dashpay = registered();
    w.lock = { ...w.lock, state: "unlockedMixingOnly" };
    return w;
  },
  /** "alice" registered before DashPay keys existed: the Enable DashPay banner (DP2-09 D10). */
  "dashpay-no-keys": (t) => {
    const w = base(t);
    w.dashpay = registered();
    w.dashpay.identities = w.dashpay.identities.map((i) => ({ ...i, hasDashpayKeys: false }));
    return w;
  },
  /** "alice" with fewer credits than one contact request: Accept is refused (DP2-09 D4). */
  "dashpay-low-credits": (t) => {
    const w = base(t);
    w.dashpay = registered();
    w.dashpay.identities = w.dashpay.identities.map((i) => ({ ...i, balance: "4000000", creditLevel: "empty" }));
    return w;
  },
  /** "alice" with nothing in the bell: no request waiting, no event (DP2-09 D12). */
  "dashpay-bell-empty": (t) => {
    const w = base(t);
    const world = registered();
    w.dashpay = { ...world, contacts: world.contacts.filter((c) => c.summary.relation !== "pendingIncoming"), journal: [] };
    return w;
  },
  /** Two wallets, each "alice" with her own contacts and unread events: the bell is the window
   * wallet's (DP2-09 D27). */
  "dashpay-two-wallets": (t) => {
    const w = twoWallets(base(t));
    w.dashpay = registered();
    w.dashpayOf = { [SECOND_WALLET_ID]: registered() };
    return w;
  },
  /** DAPI does not answer user searches: the network section fails with Try again. */
  "dashpay-search-offline": (t) => {
    const w = base(t);
    w.dashpay = { ...registered(), searchOffline: true };
    return w;
  },
  /** macOS: no screen picker; Add Contact points to the system's copy-a-region shortcut (D10). */
  "dashpay-screen-shortcut": (t) => {
    const w = base(t);
    w.dashpay = registered();
    w.scan.mode = "copyShortcut";
    return w;
  },
  /** Locking cuts more flows than the report lists, some pending (E0-13 D3): unresolved for good. */
  "lock-report-omitted": (t) => omittedPending(base(t)),
  /** The report omits flows, artifacts and wallets that its order shows are not pending. */
  "lock-report-omitted-settles": (t) => omittedSettled(base(t)),
  /** Locking cuts flows; the report lists them all, some still pending. */
  "lock-report-pending": (t) => listedFlows(base(t)),
  /** Locking cuts flows that all ended: sent or cancelled. */
  "lock-report-resolved": (t) => resolvedFlows(base(t)),
  /** A cut call reported under its label (no id to check): unresolved for good. */
  "lock-report-call": (t) => unnamedCall(base(t)),
} satisfies Record<string, (theme: ThemeName | null) => World>;

export type ScenarioName = keyof typeof SCENARIOS;
export const SCENARIO_NAMES = Object.keys(SCENARIOS) as ScenarioName[];
export const DEFAULT_SCENARIO: ScenarioName = "funded";

export function isScenario(s: string): s is ScenarioName {
  return Object.hasOwn(SCENARIOS, s);
}

export function buildWorld(name: ScenarioName, theme: ThemeName | null = null): World {
  return SCENARIOS[name](theme);
}
