// dw-fixture-module: never in a production build (vite.config.ts, noFixtureModules).
// Named fixture scenarios. Each builds a fresh `World`: the state the fixture's command
// handlers read and change. Pick one with `?scenario=<name>` (dev server, Playwright) or
// `installFixture({ scenario })` (vitest).

import type { AuthGrant } from "../api/generated/AuthGrant";
import type { ContestStatus } from "../api/generated/ContestStatus";
import type { DashPayStatus } from "../api/generated/DashPayStatus";
import type { IdentitySummary } from "../api/generated/IdentitySummary";
import type { InvitationStatus } from "../api/generated/InvitationStatus";
import type { PendingTopUp } from "../api/generated/PendingTopUp";
import type { ProtocolCompatibility } from "../api/generated/ProtocolCompatibility";
import type { QuorumSource } from "../api/generated/QuorumSource";
import type { DispatchStateName } from "../api/generated/DispatchStateName";
import type { FlowKindName } from "../api/generated/FlowKindName";
import type { FlowLockReport } from "../api/generated/FlowLockReport";
import type { FlowOutcomeName } from "../api/generated/FlowOutcomeName";
import type { LaunchInfo } from "../api/generated/LaunchInfo";
import type { LinkRoute } from "../api/generated/LinkRoute";
import { stashRoute, type FixtureStashedLink } from "./links";
import type { Lease } from "../api/generated/Lease";
import type { LockOutcome } from "../api/generated/LockOutcome";
import type { LockStatus } from "../api/generated/LockStatus";
import type { RecentNotice } from "../api/generated/RecentNotice";
import type { ShellSync } from "../api/generated/ShellSync";
import type { SyncPassReport } from "../api/generated/SyncPassReport";
import type { SyncStatus } from "../api/generated/SyncStatus";
import type { ThemeName } from "../api/generated/ThemeName";
import type { TxDropInfo } from "../api/generated/TxDropInfo";
import type { TxRow } from "../api/generated/TxRow";
import type { WalletSummary } from "../api/generated/WalletSummary";
import { contestedRow, coreActivity, IDENTITY, noIdentity, registered, registering, type DashPayWorld } from "./dashpay";
import { identitiesWorld } from "./identities";
import { newCreditsWorld, topUpStep, withdrawalId, withdrawPayout, type CreditsWorld } from "./topup";
import type { Utxo } from "../api/generated/Utxo";
import type { Withdrawal } from "../api/generated/Withdrawal";
import { buildCoins, newReceiveWorld, newSendWorld, type ReceiveWorld, type SendWorld } from "./send";
import { newToolsWorld, type ToolsWorld } from "./tools";
import { newSettingsWorld, withAutomaticBackups, withRestoreBackup, type SettingsWorld } from "./settings";
import { partlyUsedLock, restoredFundLock, restoredLock } from "./recovery";
import { newPendingWorld, PLATFORM_PAYEE, sendTerms, type PendingWorld } from "./pending";
import { scanWorld, type ScanWorld } from "./scan";
import { holdShielded } from "./shielded";
import { coinjoinScenarios, newCoinJoinWorld, type CoinJoinWorld } from "./coinjoin";
import { newRatesWorld, onMainnet, ratesScenarios, withPrices, type RatesWorld } from "./rates";
import { newUpdatesWorld, type UpdatesWorld } from "./updates";
import { governanceScenarios, newGovernanceWorld, type GovernanceWorld } from "./governance";
import { masternodesScenarios, noMasternodesWorld, type MasternodesWorld } from "./masternodes";
import { newSharedWorld, sharedScenarios, type SharedWorld } from "./shared";
import { transferScenarios } from "./transfer";
import { MAIN_PHRASE, SECOND_PHRASE, throttleWaitSecs } from "./vault";
import { COIN } from "../format/amount";
import { buildRows, caughtUp, FIXTURE_NOW, FUNDED_BALANCES, makeIds, prng, secs, XSS_PAYLOADS, ZERO_BALANCES, type HistoryVariant } from "./wallet";

export const FIXTURE_PASSPHRASE = "demo";
export const FIXTURE_WALLET_ID = "a1".repeat(32);
/** The `two-wallets` scenario's second wallet. */
export const SECOND_WALLET_ID = "b2".repeat(32);

export interface FixtureWallet {
  summary: WalletSummary;
  rows: TxRow[];
  coins: Utxo[];
  receive: ReceiveWorld;
  /** Filter checkpoint. Omitted: the tip once Core is caught up, otherwise no scan state. */
  scanHeight?: number | null;
  /** Dropped payments whose notice was dismissed. */
  droppedSeen?: string[];
}

/** A 4 x 4 PNG in the DashPay blue: the fixture's avatar thumbnail (DP4-02). */
export const AVATAR_PNG = "iVBORw0KGgoAAAANSUhEUgAAAAQAAAAECAYAAACp8Z5+AAAAEklEQVR4nGOQKd3zHxkzkC4AAMqAJMErkdEyAAAAAElFTkSuQmCC";

/** What the fixture's engine knows. Handlers mutate it, like the engine's own state. */
export interface World {
  networkForced?: boolean;
  /** Row 74: what the next `switch_network` to the other network does instead of switching: refuse
   * (`busy`, or close's own refusal `closeRefused`), or fail after the close (`fail`; when it did
   * not roll back, the next `open_network` fails once too, `reopenFails`). */
  networkSwitch?: {
    busy?: { flows: FlowKindName[]; mixing: boolean; broadcast: boolean; stalled?: boolean; draining?: boolean };
    closeRefused?: "wallet" | "shielded_spend_unresolved" | "transfer_preparing";
    fail?: { rolledBack: boolean };
    reopenFails?: boolean;
    /** The switch goes through but its choice is not written. */
    saveFails?: boolean;
  };
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
  /** The session's quorum source and the developer toggle (E0-10b; DP6-03). The default is the
   * degraded mode's `trusted` (DEC-168). */
  trust: { source: QuorumSource; trustedOnly: boolean };
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
  /** The engine's link stash (DP5-01): links kept for after onboarding, and invitations. */
  stash: FixtureStashedLink[];
  /** DP5-03: `invitation_status` of a stashed invitation by its id; absent: valid, from Bob. */
  invitations?: Record<string, InvitationStatus>;
  /** The last `shell_sync`: what the native menus would show. */
  shellSync: ShellSync | null;
  /** Txids `open_tx_in_explorer` accepted, in call order. The command takes no network. */
  explorerOpened?: string[];
  /** How many times `open_releases_page` was called. */
  releasesOpened?: number;
  /** `open_project_link` calls, in order (the fixture opens nothing). */
  linksOpened?: string[];
  /** `open_project_link` fails as when no browser opens. */
  linkOpenFails?: boolean;
  /** `protocol_compatibility`'s answer; absent: equal (`15` / `15`). */
  protocolCompatibility?: ProtocolCompatibility;
  /** "Ask for my passphrase for every payment". The funded scenario leaves it on, so the
   * existing screens keep asking. The engine's own default is off: the signing window. */
  paymentAuth: boolean;
  /** Quick-unlock and signing-window spend limit, in duffs. Default 0.5 DASH. */
  spendLimitDuffs: number;
  /** The sealed limit no longer authenticates (issue 173, item 10): while unlocked the engine reports
   * it, the limit in force is 0, and setting the limit again clears it. */
  spendLimitUnreadable?: boolean;
  /** When the post-unlock signing window ends (ms). `null` when it is closed. The funded
   * scenario starts unlocked, so the window is open. */
  signingUntil: number | null;
  /** Duff already authorized from this window. The limit is a total, not per payment. */
  windowCharged?: number;
  /** "Load contact pictures" (DP4-02 D17); absent: on. */
  loadContactPictures?: boolean;
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
  /** CoinJoin's options and each wallet's mixer (coinjoin.ts, U-06). */
  coinjoin: CoinJoinWorld;
  /** Governance: sync, proposals, voting masternodes and own proposals (governance.ts, MG-09). */
  governance: GovernanceWorld;
  /** My masternodes: the SPV list, own and tracked masternodes, attached keys (masternodes.ts, MG-10). */
  masternodes: MasternodesWorld;
  /** Shared masternodes: the capability and the main wallet's sessions (shared.ts, MG-10b). */
  shared: SharedWorld;
  /** Prices in fiat: settings, rates.json and the next attempt (rates.ts, MP-01). */
  rates: RatesWorld;
  /** Updates: what this copy can do, the feed and the install (updates.ts, PK-03). */
  updates: UpdatesWorld;
  /** The commands dw-app does not have yet: network choice, shielded and Platform balances (pending.ts). */
  pending: PendingWorld;
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

function wallet(variant: HistoryVariant, plainWords: boolean): FixtureWallet {
  const rows = buildRows(variant, plainWords);
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
    rows,
    coins: buildCoins(variant, rows.find((r) => r.amount > 0 && r.status.kind === "unconfirmed" && !r.status.instantLocked)?.txid),
    receive: newReceiveWorld(),
  };
}

/** A wallet `import_wallet` adds: no history yet, and its birth height the tip's. */
export function emptyWallet(walletId: string, name: string): FixtureWallet {
  const w = wallet("empty", false);
  w.summary = { ...w.summary, walletId, name };
  return w;
}

/** The funded wallet behind the lock screen after `failedAttempts` wrong passphrases, with the
 * wait dw-vault reports for them (the fixture's clock stands still, so it never runs out). */
function lockedAfter(w: World, failedAttempts: number): World {
  w.lock = { ...w.lock, state: "locked", failedAttempts, retryAfterSecs: throttleWaitSecs(failedAttempts) || null };
  return w;
}

/** Adds a payment Dash Wallet dropped itself for each of `drops`, newest first, an hour apart, to the
 * first wallet: an abandoned send, not counted (the never-valid transaction itself is a receive). */
function withDropped(w: World, drops: TxDropInfo[]): World {
  const wallet = w.wallets[0];
  const sent = wallet.rows.find((r) => r.category === "sent")!;
  const received = wallet.rows.find((r) => r.category === "received")!;
  const newest = Math.max(...wallet.rows.map((r) => r.timestamp ?? 0));
  const rows: TxRow[] = drops.map((dropped, i) => {
    const like = dropped.cause === "invalidClaim" ? received : sent;
    return {
      ...like,
      txid: (0xd0 + i).toString(16).repeat(32),
      recordIndex: 0,
      timestamp: newest + 3600 * (drops.length - i),
      blockHeight: null,
      amount: like.amount < 0 ? -(25_000_000 + i * 1_000_000) - 374 : 40_000_000,
      fee: like.amount < 0 ? 374 : null,
      label: null,
      status: { kind: "abandoned", confirmations: 0, instantLocked: false, chainLocked: false, maturesIn: null, dropped },
      countsTowardBalance: false,
      counterparty: undefined,
      fiatThen: undefined,
    };
  });
  wallet.rows = [...rows, ...wallet.rows];
  return w;
}

/** The txid of `withPendingUnproven`'s payment. */
export const PENDING_UNPROVEN_TXID = "e7".repeat(32);
/** Its change, which the engine withholds. */
export const PENDING_UNPROVEN_CHANGE = 74_999_774;

/** Adds to the first wallet an unconfirmed payment the engine withholds the change of: the change is
 * listed, not spendable and in no balance bucket, and the balances report it as withheld. */
function withPendingUnproven(w: World): World {
  const wallet = w.wallets[0];
  const sent = wallet.rows.find((r) => r.category === "sent")!;
  const newest = Math.max(...wallet.rows.map((r) => r.timestamp ?? 0));
  const row: TxRow = {
    ...sent,
    txid: PENDING_UNPROVEN_TXID,
    recordIndex: 0,
    timestamp: newest + 600,
    blockHeight: null,
    label: null,
    amount: -25_000_226,
    fee: 226,
    status: { kind: "unconfirmed", confirmations: 0, instantLocked: false, chainLocked: false, maturesIn: null },
  };
  wallet.rows = [row, ...wallet.rows];
  wallet.coins = [
    ...wallet.coins,
    {
      ...wallet.coins[0],
      outpoint: { txid: PENDING_UNPROVEN_TXID, vout: 1 },
      amount: PENDING_UNPROVEN_CHANGE,
      confirmations: 0,
      blockHeight: null,
      instantLocked: false,
      chainLocked: false,
      label: null,
      isChange: true,
      reserved: true,
      spendable: false,
      excluded: "pendingUnproven",
    },
  ];
  wallet.summary = { ...wallet.summary, balances: { ...(wallet.summary.balances ?? FUNDED_BALANCES), withheld: PENDING_UNPROVEN_CHANGE } };
  return w;
}

/** The txid of `withAbandonChecking`'s payment. */
export const ABANDON_CHECKING_TXID = "e8".repeat(32);

/** Adds to the first wallet a payment the user abandoned, a coin of which the engine's rescan is
 * still checking before giving it back. */
function withAbandonChecking(w: World): World {
  const wallet = w.wallets[0];
  const sent = wallet.rows.find((r) => r.category === "sent")!;
  const newest = Math.max(...wallet.rows.map((r) => r.timestamp ?? 0));
  const row: TxRow = {
    ...sent,
    txid: ABANDON_CHECKING_TXID,
    recordIndex: 0,
    timestamp: newest + 600,
    blockHeight: null,
    label: null,
    countsTowardBalance: false,
    status: { kind: "abandoned", confirmations: 0, instantLocked: false, chainLocked: false, maturesIn: null },
  };
  wallet.rows = [row, ...wallet.rows];
  w.send.checkingSpentCoins = [ABANDON_CHECKING_TXID];
  return w;
}

function base(theme: ThemeName | null, variant: HistoryVariant = "default", plainWords = false): World {
  const main = wallet(variant, plainWords);
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
      // A dev build from main, as the release workflow stamps one.
      buildVersion: "0.1.0-dev.57.g1a2b3c4",
    },
    passphrase: FIXTURE_PASSPHRASE,
    lock: unlocked([FIXTURE_WALLET_ID]),
    phrases: { [FIXTURE_WALLET_ID]: MAIN_PHRASE },
    wallets: [main],
    sync: caughtUp(),
    dashpay: noIdentity(),
    trust: { source: "trusted", trustedOnly: false },
    leases: [],
    grants: [],
    flowLeases: {},
    issued: 0,
    dispatch: {},
    lastLockReport: null,
    cut: { flows: [], flowsOmitted: 0 },
    notices: [],
    links: [],
    stash: [],
    shellSync: null,
    paymentAuth: true,
    spendLimitDuffs: 50_000_000,
    signingUntil: Date.now() + 5 * 60 * 1000,
    throttleSecs: null,
    throttledUntil: null,
    send: newSendWorld(),
    tools: newToolsWorld(main),
    settings: withAutomaticBackups(newSettingsWorld(), FIXTURE_WALLET_ID, MAIN_PHRASE, FIXTURE_PASSPHRASE),
    scan: scanWorld(),
    coinjoin: newCoinJoinWorld(),
    governance: newGovernanceWorld(),
    masternodes: noMasternodesWorld(),
    shared: newSharedWorld(),
    rates: newRatesWorld(),
    updates: newUpdatesWorld(),
    pending: newPendingWorld(),
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
    items.push(coreActivity({ txid: row.txid, direction, amount: Math.abs(row.amount) - (direction === "sent" ? (row.fee ?? 0) : 0), height: row.blockHeight, timestamp: row.timestamp }));
  };
  attribute(first("recvWithAddress")!, "bob", "received");
  attribute(first("sendToAddress")!, "dave", "sent");
}

/** `dashpay-registered` with Alice's avatar published and verified (DP4-02). */
function withAvatar(t: ThemeName | null): World {
  const w = base(t);
  const world = registered();
  const alice = world.identities.find((i) => i.identity === IDENTITY.alice)!;
  alice.profile = {
    ...alice.profile!,
    avatarUrl: "https://example.org/alice.png",
    avatarHash: "ab".repeat(32),
    avatarFingerprint: "cd".repeat(8),
  };
  w.dashpay = { ...world, avatars: { [IDENTITY.alice]: AVATAR_PNG } };
  return w;
}

/** Solid 8 x 8 PNGs (green, red, purple, orange): the other users' thumbnails (DP4-03). */
const PICTURES = [
  "iVBORw0KGgoAAAANSUhEUgAAAAgAAAAICAIAAABLbSncAAAAEUlEQVR42mPQqzXCihiGlgQAlgM3QYr963wAAAAASUVORK5CYII=",
  "iVBORw0KGgoAAAANSUhEUgAAAAgAAAAICAIAAABLbSncAAAAEUlEQVR42mM4pqGBFTEMLQkAT91FgcmWcpUAAAAASUVORK5CYII=",
  "iVBORw0KGgoAAAANSUhEUgAAAAgAAAAICAIAAABLbSncAAAAEUlEQVR42mPIkp6FFTEMLQkA/V1HwaOKzf8AAAAASUVORK5CYII=",
  "iVBORw0KGgoAAAANSUhEUgAAAAgAAAAICAIAAABLbSncAAAAEUlEQVR42mN4n8OAFTEMLQkAIYZWwaEHDpcAAAAASUVORK5CYII=",
];

/** `dashpay-avatar` with the history's counterparties (DP3-07) and everyone's picture but
 * Carol's; Erin's bytes do not decode. `on`: "Load contact pictures". */
function withPictures(t: ThemeName | null, on: boolean): World {
  const w = withAvatar(t);
  attributeMore(w);
  const others = Object.entries(IDENTITY).filter(([name]) => name !== "alice" && name !== "carol");
  const avatars = Object.fromEntries(others.map(([name, id], i) => [id, name === "erin" ? "bm90IGEgcGljdHVyZQ==" : PICTURES[i % PICTURES.length]]));
  w.dashpay.avatars = { ...w.dashpay.avatars, ...avatars };
  w.loadContactPictures = on;
  return w;
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
  w.dashpay.activity[IDENTITY.bob].unshift(coreActivity({ txid: refund.txid, direction: "received", amount: refund.amount, height: refund.blockHeight, timestamp: refund.timestamp }));
  const nameless = rows.find((r) => r.txType === "recvWithAddress" && r.category === "received" && !r.label && !r.counterparty && r.status.kind === "confirmed")!;
  nameless.counterparty = { identity: IDENTITY.mateo, username: null, displayName: null, avatarUrl: null };
  // The oldest plain payment went to Judy, a contact the user hid: listed, in the collapsed Hidden section.
  const hidden = rows.findLast((r) => r.txType === "sendToAddress" && !r.label && !r.counterparty && r.status.kind === "confirmed")!;
  hidden.counterparty = { identity: IDENTITY.judy, username: "judy", displayName: "Judy", avatarUrl: null };
  (w.dashpay.activity[IDENTITY.judy] ??= []).push(coreActivity({ txid: hidden.txid, direction: "sent", amount: Math.abs(hidden.amount) - (hidden.fee ?? 0), height: hidden.blockHeight, timestamp: hidden.timestamp }));
}

/** Every wallet of `w` born below block 1,100,000, where its stored headers begin: Home's
 * history-gap notice names them all. */
export function withHistoryGap(w: World): World {
  for (const x of w.wallets) x.summary.birthHeight = 1_000_000;
  w.tools.historyGap = { startHeight: 1_100_000, walletIds: w.wallets.map((x) => x.summary.walletId) };
  return w;
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

/** "alice" registered, her summary patched (DP1-10). */
function withAlice(w: World, patch: Partial<IdentitySummary>): World {
  w.dashpay = registered();
  w.dashpay.identities = w.dashpay.identities.map((i) => ({ ...i, ...patch }));
  return w;
}

/** "alice" registered with this credits state (DP1-10). */
function withCredits(w: World, patch: Partial<CreditsWorld>): World {
  w.dashpay = { ...registered(), credits: newCreditsWorld(patch) };
  return w;
}

/** A top-up lock of 0.05 Dash for "alice" (DP1-10). */
const topUpLock = (patch: Partial<PendingTopUp>): PendingTopUp => ({
  txid: "a1".repeat(32),
  identity: IDENTITY.alice,
  identityIndex: 0,
  duffs: 5_000_000,
  proved: false,
  consumptionUnknown: false,
  ...patch,
});

/** Where "alice"'s past withdrawals paid (DP6-02). */
const FIXTURE_PAYOUT_TO = "yNPbcFfabtNmmxKdGwhHomdYfVs6gikbPf";

/** One of "alice"'s withdrawal documents (DP6-02), `hoursAgo` hours before the fixture's now. */
function withdrawal(n: number, status: Withdrawal["status"], credits: number, hoursAgo: number): Withdrawal {
  const at = FIXTURE_NOW.getTime() - hoursAgo * 3_600_000;
  return { id: withdrawalId(n), credits: String(credits), expectedDuffs: withdrawPayout(BigInt(credits)), to: FIXTURE_PAYOUT_TO, status, createdAt: at, updatedAt: at };
}

/** The contested world with `contest_status` reading `state` (DP1-10 R3–R5). */
function withContest(w: World, state: ContestStatus["state"], temporaryName = "alice007"): World {
  const s = w.dashpay.status;
  if (s.kind !== "contestPending") return w;
  const contest: ContestStatus = {
    label: s.label,
    state,
    endsAt: s.endsAt,
    contenders: [
      { identity: IDENTITY.alice, votes: 31, isSelf: true },
      { identity: IDENTITY.mallory, votes: 40, isSelf: false },
    ],
    lockVotes: state.kind === "locked" ? 55 : 3,
    abstainVotes: 2,
    temporaryName,
  };
  w.dashpay.credits = newCreditsWorld({ contest });
  return w;
}

/** "alice" contested by a registration that asked for the temporary "alice2" beside it (DP1-09
 * D4 reversed): registered (`owned`), or refused, so the identity holds no name. */
function withInstant(w: World, owned: boolean): World {
  const c = contestPending(w, 1_900_000_000);
  c.dashpay.registrations = [contestedRow("alice2")];
  c.dashpay.identities = c.dashpay.identities.map((i) => (owned ? { ...i, names: ["alice2"], mainName: "alice2" } : { ...i, names: [], mainName: null }));
  return c;
}

/** The wallet's DashPay status set to `status`, no identity on file (DP1-08). The bring-up's
 * outcome says the same: a `startupIncomplete` status's own; a watch-only wallet's bring-up does
 * not run and is read-only (bringup.rs `admit`). Tools ▸ Information reads both (DP6-03). */
function withStatus(w: World, status: DashPayStatus): World {
  const d = noIdentity();
  if (status.kind === "startupIncomplete") d.startup = { ...d.startup, startup: status.startup };
  if (status.kind === "noIdentity" && status.reason === "watchOnly") d.startup = { ...d.startup, startup: "notRun", readOnly: true, finishedAt: null };
  w.dashpay = { ...d, status };
  return w;
}

/** DP6-03: `dashpay-registered` with the session's quorum source, the toggle and a sync report. */
function withSyncInfo(t: ThemeName | null, trust: World["trust"], lastPass: SyncPassReport | null = null): World {
  const w = base(t);
  w.dashpay = registered();
  w.trust = trust;
  if (lastPass) w.dashpay.lastPass = lastPass;
  return w;
}

/** A pass that ended 5 s before FIXTURE_NOW after 3 s. */
const pass = (found: Partial<SyncPassReport>): SyncPassReport => ({
  startedAt: secs(FIXTURE_NOW) - 8,
  finishedAt: secs(FIXTURE_NOW) - 5,
  newRequests: 0,
  newContacts: 0,
  newPayments: 0,
  failureCode: null,
  ...found,
});

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

/** The `saved-links` scenario's links: a testnet payee and a contact's identity. */
const SAVED_PAYEE = "yNPbcFfabtNmmxKdGwhHomdYfVs6gikbPf";
const SAVED_CONTACT = "4EfA9Jrvv3nnCFdSf7fad59851iiTRZ6Wcu6YVJ4iSeF";

/** `w` with one invitation in its stash, opened a day ago; `status` its `invitation_status`. */
function withInvitation(w: World, status?: InvitationStatus): World {
  const now = Math.floor(Date.now() / 1000);
  const { id } = stashRoute(w.stash, { kind: "invitation", linkId: "" }, now - 86_400);
  if (status) w.invitations = { [id]: status };
  return w;
}

/** Platform and Shielded history succeed with nothing. A failed command is a different state. */
function quietOptionalHistory(w: World): World {
  if (w.pending.platform) w.pending.platform.activity = [];
  if (w.pending.shielded) w.pending.shielded.activity = [];
  return w;
}

/** A new wallet still catching up when a 25 DASH payment arrives, InstantSend-locked or only seen. */
function catchingUpPayment(t: ThemeName | null, locked: boolean): World {
  const w = base(t, "empty");
  const paid = 25 * COIN;
  const main = w.wallets[0];
  const arrived = locked ? { confirmed: paid } : { unconfirmed: paid };
  main.summary = { ...main.summary, balances: { ...ZERO_BALANCES, ...arrived, total: paid, catchingUp: true } };
  main.rows = [
    {
      txid: "5f0c2a6e9d4b4a3c8e7f1a2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d7e8f9a0b1c2d",
      recordIndex: 0,
      txType: "recvWithAddress",
      category: "received",
      status: { kind: "unconfirmed", confirmations: 0, instantLocked: locked, chainLocked: false, maturesIn: null },
      timestamp: secs(FIXTURE_NOW),
      blockHeight: null,
      amount: paid,
      fee: null,
      address: "yXbH3JxbVWgEzq6aZDyEbNQx5PtUfZvvXZ",
      label: null,
      countsTowardBalance: locked,
      involvesWatchOnly: false,
    },
  ];
  w.sync = syncing();
  return w;
}

/** A wallet whose own scan trails the tip by 2,000 blocks: SPV stopped short, or the network caught
 * up while this wallet still scans. `empty`: nothing found so far, no Shielded or Platform pool. */
function walletBehind(t: ThemeName | null, stopped: boolean, empty: boolean): World {
  const w = empty ? base(t, "empty") : base(t);
  if (empty) {
    w.pending.shielded = null;
    w.pending.platform = null;
  }
  const tip = w.sync.tipHeight!;
  w.wallets[0].scanHeight = tip - 2000;
  if (stopped) w.sync = { ...syncing(), running: false, connectedPeers: 0 };
  return w;
}

const SCENARIOS = {
  /** The reference wallet: balance, a month of history, caught up, unlocked. */
  funded: (t) => base(t, "default", true),
  /** The first run: the pre-release notice is up (the other scenarios start past it). */
  "pre-release-notice": (t) => {
    const w = base(t);
    w.settings.ui = { ...w.settings.ui, preReleaseNoticeDismissed: false };
    return w;
  },
  /** The first run on a new install: no wallet, and the notice is up. */
  "no-wallet-first-run": (t) => {
    const w = base(t);
    w.wallets = [];
    w.phrases = {};
    w.lock = { ...unlocked([]), state: "noVault", encrypted: false };
    w.settings.ui = { ...w.settings.ui, preReleaseNoticeDismissed: false };
    return w;
  },
  /** The first run after the vault exists but settings.json does not: locked, the notice is up. */
  "locked-first-run": (t) => {
    const w = buildWorld("locked", t);
    w.settings.ui = { ...w.settings.ui, preReleaseNoticeDismissed: false };
    return w;
  },
  /** A window started with `--network regtest` and a wallet: the choice only saves for the next start (row 74). */
  "network-pinned": (t) => {
    const w = base(t);
    w.launch = { ...w.launch, network: { kind: "regtest", dirName: "regtest", unitName: "DASH" } };
    w.pending.networkChoice = { chosen: "mainnet", saved: true };
    return w;
  },
  /** The first run on a new install, past the notice: no choice saved, so the window is on mainnet (row 74). */
  "no-wallet-mainnet": (t) => {
    const w = base(t);
    w.wallets = [];
    w.phrases = {};
    w.lock = { ...unlocked([]), state: "noVault", encrypted: false };
    w.launch = { ...w.launch, network: { kind: "mainnet", dirName: "mainnet", unitName: "DASH" } };
    w.pending.networkChoice = { chosen: "mainnet", saved: false };
    return w;
  },
  /** The funded wallet when dw-app has no shielded or Platform command: Home has neither row, and no breakdown. */
  "pools-absent": (t) => {
    const w = base(t);
    w.pending.shielded = null;
    w.pending.platform = null;
    return w;
  },
  /** One payment Dash Wallet dropped because the Dash it used came from an incoming transaction that
   * was never valid, its coins back: Home's notice names it. */
  "dropped-one": (t) => withDropped(base(t), [{ cause: "invalidSource", state: { kind: "released" } }]),
  /** Two such payments: the notice counts them, and Details opens Activity. */
  "dropped-several": (t) =>
    withDropped(base(t), [
      { cause: "invalidSource", state: { kind: "released" } },
      { cause: "invalidSource", state: { kind: "released" } },
    ]),
  /** A dropped row in each state the engine gives, and the never-valid transaction itself; no
   * notice, since none is back. */
  "dropped-states": (t) =>
    withDropped(base(t), [
      { cause: "invalidSource", state: { kind: "releasing" } },
      { cause: "invalidSource", state: { kind: "rescanNeeded" } },
      { cause: "invalidSource", state: { kind: "rescanFromHeight", height: 2_103_500 } },
      { cause: "invalidSource", state: { kind: "awaitingFunding" } },
      { cause: "replaced", state: { kind: "held" } },
      { cause: "invalidClaim", state: { kind: "released" } },
    ]),
  /** An unconfirmed payment the engine can't yet tell will confirm (Q100 E1): its change is
   * withheld, out of every balance and of spending, and shown as pending; Abandon is offered with
   * its hint. */
  "pending-unproven": (t) => withPendingUnproven(base(t)),
  /** A payment the user abandoned while the engine's rescan checks a coin it spent (Q100 E2): its
   * details say so until the check ends. */
  "abandon-checking": (t) => withAbandonChecking(base(t)),
  /** An InstantSend-locked payment a ChainLocked spend beat, sent before a restart: Abandon is
   * offered, and refused until the wallet restarts and rescans. */
  "abandon-rescan-first": (t) => {
    const w = base(t);
    const wallet = w.wallets[0];
    const sent = wallet.rows.find((r) => r.category === "sent")!;
    const row: TxRow = {
      ...sent,
      txid: "5e".repeat(32),
      timestamp: (sent.timestamp ?? 0) + 60,
      blockHeight: null,
      label: null,
      status: { kind: "unconfirmed", confirmations: 0, instantLocked: true, chainLocked: false, maturesIn: null },
    };
    wallet.rows = [row, ...wallet.rows];
    w.send.rescanFirst = [row.txid];
    return w;
  },
  /** Just after a chain fork, while the wallet catches up: an unconfirmed payment offers Abandon,
   * and Abandon and Remove Unconfirmed are refused with `rescan_in_progress`. */
  "abandon-catching-up": (t) => {
    const w = base(t);
    const wallet = w.wallets[0];
    const sent = wallet.rows.find((r) => r.category === "sent")!;
    const row: TxRow = {
      ...sent,
      txid: "5b".repeat(32),
      timestamp: (sent.timestamp ?? 0) + 60,
      blockHeight: null,
      label: null,
      status: { kind: "unconfirmed", confirmations: 0, instantLocked: false, chainLocked: false, maturesIn: null },
    };
    wallet.rows = [row, ...wallet.rows];
    w.send.catchingUp = true;
    return w;
  },
  /** An unconfirmed payment whose later payment locked while its detail was open: Abandon is
   * offered and refused, and the detail says why. */
  "abandon-settled-child": (t) => {
    const w = base(t);
    const wallet = w.wallets[0];
    const sent = wallet.rows.find((r) => r.category === "sent")!;
    const row: TxRow = {
      ...sent,
      txid: "5c".repeat(32),
      timestamp: (sent.timestamp ?? 0) + 60,
      blockHeight: null,
      label: null,
      status: { kind: "unconfirmed", confirmations: 0, instantLocked: false, chainLocked: false, maturesIn: null },
    };
    wallet.rows = [row, ...wallet.rows];
    w.send.settledChild = [row.txid];
    return w;
  },
  /** An unconfirmed payment of the first of two open wallets, whose saved transaction doesn't read:
   * Abandon is offered and refused, and so is Remove Unconfirmed; both name the wallet and say to
   * close it, or remove and restore it. */
  "abandon-record-unreadable-two-wallets": (t) => {
    const w = twoWallets(base(t));
    const wallet = w.wallets[0];
    const sent = wallet.rows.find((r) => r.category === "sent")!;
    const row: TxRow = {
      ...sent,
      txid: "5f".repeat(32),
      timestamp: (sent.timestamp ?? 0) + 60,
      blockHeight: null,
      label: null,
      status: { kind: "unconfirmed", confirmations: 0, instantLocked: false, chainLocked: false, maturesIn: null },
    };
    wallet.rows = [row, ...wallet.rows];
    w.send.recordUnreadable = [row.txid];
    return w;
  },
  /** As `abandon-record-unreadable-two-wallets`, with only that wallet open: the refusal doesn't
   * say to close it. */
  "abandon-record-unreadable": (t) => {
    const w = base(t);
    const wallet = w.wallets[0];
    const sent = wallet.rows.find((r) => r.category === "sent")!;
    const row: TxRow = {
      ...sent,
      txid: "5f".repeat(32),
      timestamp: (sent.timestamp ?? 0) + 60,
      blockHeight: null,
      label: null,
      status: { kind: "unconfirmed", confirmations: 0, instantLocked: false, chainLocked: false, maturesIn: null },
    };
    wallet.rows = [row, ...wallet.rows];
    w.send.recordUnreadable = [row.txid];
    return w;
  },
  /** An unconfirmed payment a later payment spends from, already locked when the detail opens:
   * Abandon isn't offered. */
  "abandon-settled-child-hidden": (t) => {
    const w = base(t);
    const wallet = w.wallets[0];
    const sent = wallet.rows.find((r) => r.category === "sent")!;
    const row: TxRow = {
      ...sent,
      txid: "5d".repeat(32),
      timestamp: (sent.timestamp ?? 0) + 60,
      blockHeight: null,
      label: null,
      status: { kind: "unconfirmed", confirmations: 0, instantLocked: false, chainLocked: false, maturesIn: null },
    };
    wallet.rows = [row, ...wallet.rows];
    w.send.settledChildHidden = [row.txid];
    return w;
  },
  /** A payment this wallet made on another device, seen only by its change before the coins it
   * spent are known: its debit is unknown, so Abandon warns about paying twice. */
  "abandon-debit-unknown": (t) => {
    const w = base(t);
    const wallet = w.wallets[0];
    const received = wallet.rows.find((r) => r.category === "received" && r.status.kind === "unconfirmed")!;
    const row: TxRow = { ...received, txid: "d0".repeat(32), timestamp: (received.timestamp ?? 0) + 60, label: null };
    wallet.rows = [row, ...wallet.rows];
    w.send.debitUnknown = [row.txid];
    return w;
  },
  /** The network does not take shielded transitions yet: Home says so, and the actions are gone. */
  "shielded-inactive": (t) => {
    const w = base(t);
    w.pending.shielded!.active = false;
    return w;
  },
  /** Platform addresses holding only what is too small to move: Home counts none of it. */
  "platform-dust": (t) => {
    const w = base(t);
    w.pending.platform!.addresses = [{ address: w.pending.platform!.addresses[0].address, credits: "1500000" }];
    return w;
  },
  /** Movable Platform credits and a remainder on another address too small to move. */
  "platform-partly-dust": (t) => {
    const w = base(t);
    w.pending.platform!.dust = "1500000";
    return w;
  },
  /** A testnet wallet's Platform addresses: 0.004005 tDASH (400500 duffs), none of it movable, and
   * nothing pending. */
  "platform-dust-small": (t) => {
    const w = base(t);
    w.pending.platform!.addresses = [];
    w.pending.platform!.dust = "400500000";
    return w;
  },
  /** The next send to a Platform address gets no answer from Platform: it stays pending. */
  "platform-send-pending": (t) => {
    const w = base(t);
    w.pending.platform!.nextSend = "pending";
    return w;
  },
  /** The next send is not taken in time (failed by height): it may still land. Its retry is taken. */
  "platform-send-failed": (t) => {
    const w = base(t);
    w.pending.platform!.nextSend = "failed";
    w.pending.platform!.thenSend = "done";
    return w;
  },
  /** A send that failed by height and may still land (0.01 to the fixture's payee): only its exact retry is taken. */
  "platform-send-failed-may-land": (t) => {
    const w = base(t);
    const p = w.pending.platform!;
    const at = Math.floor(FIXTURE_NOW.getTime() / 1000);
    p.activity = [{ id: "ps-held", kind: "send", state: "failed", credits: "1000000000", address: null, to: PLATFORM_PAYEE, expectedDuffs: null, feeCredits: null, at: at - 600, mayLand: true, estimated: false }, ...p.activity];
    p.terms["ps-held"] = sendTerms(PLATFORM_PAYEE, 1_000_000_000n);
    return w;
  },
  /** A move that failed by height and may still land: Move stays offered (a move at the same inputs follows it). */
  "platform-move-failed-may-land": (t) => {
    const w = base(t);
    const p = w.pending.platform!;
    const at = Math.floor(FIXTURE_NOW.getTime() / 1000);
    p.activity = [{ id: "pm-held", kind: "moveToCore", state: "failed", credits: "40000000000", address: null, to: p.activity[0].address, expectedDuffs: 39_998_000, feeCredits: null, at: at - 600, mayLand: true, estimated: false }, ...p.activity];
    return w;
  },
  /** As `platform-move-failed-may-land`, with 0.1 more on an address the held move does not take: its exit moves the 0.4. */
  "platform-move-held-and-rest": (t) => {
    const w = base(t);
    const p = w.pending.platform!;
    const at = Math.floor(FIXTURE_NOW.getTime() / 1000);
    p.activity = [{ id: "pm-held", kind: "moveToCore", state: "failed", credits: "40000000000", address: null, to: p.activity[0].address, expectedDuffs: 39_998_000, feeCredits: null, at: at - 600, mayLand: true, estimated: false }, ...p.activity];
    p.heldInputs = { "pm-held": p.addresses.map((a) => a.address) };
    p.addresses.push({ address: "tdash1q3rest7y5v9nmp3dfz0ht6l2wrk8s4xj7ga5c9e3v0u6y2pnt4qk5m8zr", credits: "10000000000" });
    return w;
  },
  /** A move and its retry at the same inputs, both failed by height and able to land: one hold of two records. */
  "platform-move-held-several": (t) => {
    const w = base(t);
    const p = w.pending.platform!;
    const at = Math.floor(FIXTURE_NOW.getTime() / 1000);
    const row = { kind: "moveToCore", state: "failed", credits: "40000000000", address: null, to: p.activity[0].address, expectedDuffs: 39_998_000, feeCredits: null, mayLand: true, estimated: false } as const;
    p.activity = [{ ...row, id: "pm-held-2", at: at - 300 }, { ...row, id: "pm-held", at: at - 600 }, ...p.activity];
    return w;
  },
  /** The next "Add from Dash wallet" is applied (as the base world does). */
  "paddr-fund-applies": (t) => {
    const w = base(t);
    w.pending.platform!.nextFund = "done";
    return w;
  },
  /** Credits moved from Shielded to the receive address: a "From Shielded" row, done. */
  "paddr-from-shielded": (t) => {
    const w = base(t);
    const p = w.pending.platform!;
    p.activity = [{ id: "from-shielded-5a5a", kind: "fromShielded", state: "done", credits: "25000000000", address: p.receive.address, to: null, expectedDuffs: null, feeCredits: null, at: Math.floor(FIXTURE_NOW.getTime() / 1000) - 600, mayLand: false, estimated: false }, ...p.activity];
    return w;
  },
  /** A lock of "Add from Dash wallet" waiting for its proof: the card shows Finish. */
  "paddr-fund-proof-waiting": (t) => {
    const w = base(t);
    const p = w.pending.platform!;
    const txid = "f0aa".padEnd(64, "d");
    p.funds = [{ txid, duffs: 50_000_000, proved: false, consumptionUnknown: false, state: "confirming" }];
    p.activity = [{ id: `fund-${txid}`, kind: "fromCore", state: "pending", credits: String(50_000_000n * 1000n - 56_000_000n), address: p.receive.address, to: null, expectedDuffs: null, feeCredits: null, at: Math.floor(FIXTURE_NOW.getTime() / 1000) - 600, mayLand: false, estimated: true }, ...p.activity];
    return w;
  },
  /** The next "Add from Dash wallet" locks, then its approval ends before the apply: "Locked, not applied" offers Finish. */
  "paddr-fund-approval-ended": (t) => {
    const w = base(t);
    w.pending.platform!.nextFund = "approvalEnded";
    return w;
  },
  /** A lock waiting to be applied whose first Finish is refused `platform.needs_grant`: the refusal offers Finish on it. */
  "paddr-fund-resume-ended": (t) => {
    const w = base(t);
    const p = w.pending.platform!;
    p.funds = [{ txid: "f0dd".padEnd(64, "d"), duffs: 50_000_000, proved: true, consumptionUnknown: false, state: "ready" }];
    p.nextResume = "needsGrant";
    return w;
  },
  /** A lock that may already be applied (`consumption_unknown`): "Checking", and Check again. */
  "paddr-fund-checking": (t) => {
    const w = base(t);
    const p = w.pending.platform!;
    const txid = "f0bb".padEnd(64, "d");
    p.funds = [{ txid, duffs: 50_000_000, proved: true, consumptionUnknown: true, state: "checking" }];
    p.activity = [{ id: `fund-${txid}`, kind: "fromCore", state: "pending", credits: String(50_000_000n * 1000n - 56_000_000n), address: p.receive.address, to: null, expectedDuffs: null, feeCredits: null, at: Math.floor(FIXTURE_NOW.getTime() / 1000) - 600, mayLand: false, estimated: true }, ...p.activity];
    return w;
  },
  /** An own lock worth less than the Platform fee: the card says why it can't be added, with no Finish, and a new add is allowed. */
  "paddr-fund-too-small": (t) => {
    const w = base(t);
    const p = w.pending.platform!;
    const txid = "f0cc".padEnd(64, "d");
    p.funds = [{ txid, duffs: 5_000, proved: true, consumptionUnknown: false, state: "tooSmall" }];
    return w;
  },
  /** A send Platform has not confirmed: Pay from Platform and Move wait for it. */
  "platform-send-held": (t) => {
    const w = base(t);
    const p = w.pending.platform!;
    const at = Math.floor(FIXTURE_NOW.getTime() / 1000);
    p.activity = [{ id: "ps-held", kind: "send", state: "pending", credits: "1000000000", address: null, to: PLATFORM_PAYEE, expectedDuffs: null, feeCredits: null, at: at - 600, mayLand: true, estimated: false }, ...p.activity];
    return w;
  },
  /** No credits on the Platform addresses: Pay from Platform says so. */
  "platform-empty": (t) => {
    const w = base(t);
    w.pending.platform!.addresses = [];
    w.pending.platform!.activity = [];
    return w;
  },
  /** A move the engine replaced with a retry (done), among the Platform activity. */
  "platform-replaced": (t) => {
    const w = base(t);
    const p = w.pending.platform!;
    const at = Math.floor(FIXTURE_NOW.getTime() / 1000);
    p.activity = [
      { id: "pm-retry", kind: "moveToCore", state: "done", credits: "30000000000", address: null, to: p.activity[0].address, expectedDuffs: 29_998_000, feeCredits: "2000000", at: at - 3600, mayLand: false, estimated: false },
      { id: "pm-old", kind: "moveToCore", state: "replaced", credits: "30000000000", address: null, to: null, expectedDuffs: null, feeCredits: null, at: at - 7200, mayLand: false, estimated: false },
      ...p.activity,
    ];
    return w;
  },
  /** Groups that settled with no evidence of which member went through, as the engine marks them: two moves
   * of other terms at the same inputs, and two exits of a top-up that did not land, of other amounts. Only
   * each group's newest row carries its debit. */
  "platform-settled-unknown": (t) => {
    const w = base(t);
    const p = w.pending.platform!;
    const at = Math.floor(FIXTURE_NOW.getTime() / 1000);
    const row = { address: null, to: null, expectedDuffs: null, feeCredits: null, mayLand: false, estimated: false } as const;
    p.activity = [
      { ...row, id: "pm-exit2", kind: "moveToCore", state: "done", credits: "20000000000", at: at - 1800, winnerKnown: false },
      { ...row, id: "pm-exit1", kind: "moveToCore", state: "failed", credits: "19000000000", at: at - 2100, winnerKnown: false },
      { ...row, id: "pt-topup", kind: "toIdentity", state: "failed", credits: "20000000000", at: at - 2400 },
      { ...row, id: "pm-move", kind: "moveToCore", state: "done", credits: "30000000000", at: at - 3600, winnerKnown: false },
      { ...row, id: "pm-old", kind: "moveToCore", state: "replaced", credits: "25000000000", at: at - 7200, winnerKnown: false },
      ...p.activity,
    ];
    return w;
  },
  /** A wallet that starts one block short of the tip and has never been caught up. */
  "sync-one-behind": (t) => {
    const w = base(t);
    const s = w.sync;
    w.sync = { ...s, caughtUp: false, activePhase: "filters", phases: s.phases.map((p) => ({ ...p, currentHeight: p.targetHeight! - 1, done: false })) };
    return w;
  },
  /** Shielded ready and funded while the wallet's Core sync still runs: a shield's funding waits. */
  "shielded-core-syncing": (t) => {
    const w = base(t);
    w.sync = syncing();
    return w;
  },
  /** Shielded is on and its account is not established yet: its actions wait and say why, the
   * status bar says so with the first scan's progress, and Receive shows the address already. */
  "shielded-syncing": (t) => {
    const w = base(t);
    w.pending.shielded!.ready = false;
    w.pending.shielded!.balance = "0";
    w.pending.shielded!.scan = { checked: 42_000, total: 100_000 };
    return w;
  },
  /** Shielded's first bind needs an unlock: Receive says so and shows no address; the bar is Synced,
   * Sync progress says "Unlock to sync"; Send, Add and Transfer name the unlock. The vault itself reads
   * unlocked here: the app shows the lock screen while it is locked, so this shows the engine's
   * answer alone. */
  "shielded-locked": (t) => {
    const w = base(t);
    w.pending.shielded!.ready = false;
    w.pending.shielded!.balance = "0";
    w.pending.shielded!.account = "locked";
    return w;
  },
  /** The same while the wallet is unlocked for mixing only: a full unlock binds it. */
  "shielded-mixing-only": (t) => {
    const w = base(t);
    w.pending.shielded!.ready = false;
    w.pending.shielded!.balance = "0";
    w.pending.shielded!.account = "mixingOnly";
    w.lock = { ...w.lock, state: "unlockedMixingOnly" };
    return w;
  },
  /** A watch-only wallet has no viewing key to bind. */
  "shielded-watch-only": (t) => {
    const w = base(t);
    w.pending.shielded!.ready = false;
    w.pending.shielded!.balance = "0";
    w.pending.shielded!.account = "watchOnly";
    return w;
  },
  /** The wallet's last Platform-address pass failed: the bar says Platform did not finish. */
  "platform-addresses-failed": (t) => {
    const w = base(t);
    w.dashpay.addresses = "failed";
    return w;
  },
  /** Shielded's sync status cannot be read: the bar says so, never Synced. */
  "shielded-sync-unread": (t) => {
    const w = base(t);
    w.pending.shielded!.syncInfoFails = true;
    return w;
  },
  /** Shielded is established and its last pass failed: the bar says it did not finish. */
  "shielded-pass-failed": (t) => {
    const w = base(t);
    w.pending.shielded!.lastPassFailed = true;
    return w;
  },
  /** Shielded is not ready and an earlier shield is waiting to be finished. */
  "shielded-syncing-pending": (t) => {
    const w = base(t);
    w.pending.shielded!.ready = false;
    w.pending.shielded!.balance = "0";
    w.pending.shielded!.pendingShields = [{ txid: "c0ffee00".repeat(8), duffs: 50_000_000, proved: false, consumptionUnknown: false }];
    return w;
  },
  /** A shield lock that may already be applied (`consumption_unknown`): "Checking", and Check again. */
  "shielded-checking": (t) => {
    const w = base(t);
    w.pending.shielded!.pendingShields = [{ txid: "c0ffee01".repeat(8), duffs: 50_000_000, proved: true, consumptionUnknown: true }];
    return w;
  },
  /** A spend of unknown result whose record was lost (DEC-1057): sends and moves need a confirmation. */
  "shielded-unrecorded": (t) => {
    const w = base(t);
    w.pending.shielded!.unrecorded = [{ activityId: "sh-9", since: Math.floor(FIXTURE_NOW.getTime() / 1000) - 3600 }];
    return w;
  },
  /** A payment Platform refused at broadcast may still go through: its notes are held, out of the
   * balance, and Activity shows it as "May still go through". */
  "shielded-held": (t) => {
    const w = base(t);
    holdShielded(w.pending.shielded!, 1);
    return w;
  },
  /** Two such payments. */
  "shielded-held-two": (t) => {
    const w = base(t);
    holdShielded(w.pending.shielded!, 2);
    return w;
  },
  /** Shielded is ready and holds nothing: Send and Move wait for a balance. */
  "shielded-empty": (t) => {
    const w = base(t);
    w.pending.shielded!.balance = "0";
    return w;
  },
  /** Shielded is ready and a sync pass is running: the send sheet says "Scanning…". */
  "shielded-scanning": (t) => {
    const w = base(t);
    w.pending.shielded!.scanning = true;
    return w;
  },
  /** Shielded's last completed pass ended three hours before the fixture's now. */
  "shielded-synced-earlier": (t) => {
    const w = base(t);
    w.pending.shielded!.lastSyncAt = Math.floor(FIXTURE_NOW.getTime() / 1000) - 3 * 3600;
    return w;
  },
  /** Shielded's 1.25 is three notes (1, 0.2 and 0.05), priced per action: Max takes all three. */
  "shielded-three-notes": (t) => {
    const w = base(t);
    w.pending.shielded!.notes = ["100000000000", "20000000000", "5000000000"];
    return w;
  },
  /** Shielded holds less than a send's fee: Max has nothing to send and says why. */
  "shielded-below-fee": (t) => {
    const w = base(t);
    w.pending.shielded!.balance = "15000000";
    return w;
  },
  /** Shielded has too few notes to withdraw from. */
  "shielded-pool-small": (t) => {
    const w = base(t);
    w.pending.shielded!.poolTooSmall = true;
    return w;
  },
  /** One restored Core → Shielded lock of 0.5 DASH that Platform has not applied: Home offers Finish. */
  "recovery-one": (t) => {
    const w = base(t);
    w.pending.recovery.locks = [restoredLock(1, 50_000_000)];
    return w;
  },
  /** Three restored locks: two ready, one that may already be applied. */
  "recovery-restored-mixed": (t) => {
    const w = base(t);
    w.pending.recovery.locks = [restoredLock(1, 50_000_000), restoredLock(2, 25_000_000), restoredLock(3, 10_000_000, "checking")];
    return w;
  },
  /** Only a restored lock that may already be applied: "Checking", and Check again (the second one applies it). */
  "recovery-checking": (t) => {
    const w = base(t);
    w.pending.recovery.locks = [restoredLock(1, 50_000_000, "checking")];
    w.pending.recovery.checksBeforeApply = 1;
    return w;
  },
  /** Two restored locks that may already be applied: the card's plural Checking line. */
  "recovery-checking-two": (t) => {
    const w = base(t);
    w.pending.recovery.locks = [restoredLock(1, 50_000_000, "checking"), restoredLock(2, 25_000_000, "checking")];
    w.pending.recovery.checksBeforeApply = 1;
    return w;
  },
  /** A restore read back a Core → Shielded lock Platform already took: it may already be finished, and Check again hears the network say it was added. */
  "recovery-restored-finished": (t) => {
    const w = base(t);
    const lock = restoredLock(1, 50_000_000, "checking");
    w.pending.recovery.locks = [lock];
    w.pending.recovery.consumed = [lock.txid];
    return w;
  },
  /** A restore read back a Core → Shielded lock Platform has not taken: it reads checking, and the first Check again or Finish transfers applies it. */
  "recovery-restored-unapplied": (t) => {
    const w = base(t);
    w.pending.recovery.locks = [restoredLock(1, 50_000_000, "checking")];
    w.pending.recovery.checksBeforeApply = 0;
    return w;
  },
  /** A restored lock still waiting for its confirmations: Home has no card. */
  "recovery-confirming": (t) => {
    const w = base(t);
    w.pending.recovery.locks = [restoredLock(1, 50_000_000, "confirming")];
    return w;
  },
  /** A restored lock on a network that does not take Shielded yet: Home has no card. */
  "recovery-not-active": (t) => {
    const w = base(t);
    w.pending.recovery.locks = [restoredLock(1, 50_000_000, "notActive")];
    return w;
  },
  /** A ready restored lock, its transaction's second credit output (which this version can't finish) and
   * a lock worth less than the Platform fee: Home offers Finish for the first only. */
  "recovery-unsupported": (t) => {
    const w = base(t);
    const first = restoredLock(1, 50_000_000);
    w.pending.recovery.locks = [first, { ...first, vout: 1, duffs: 20_000_000, state: "unsupported" }, { ...restoredLock(2, 5_000), state: "tooSmall" }];
    return w;
  },
  /** One restored "Add from Dash wallet" lock of 0.5 DASH that Platform has not applied: Home offers Finish. */
  "paddr-recovery-restored": (t) => {
    const w = base(t);
    w.pending.recovery.locks = [restoredFundLock(1, 50_000_000)];
    return w;
  },
  /** A restore read back an "Add from Dash wallet" lock Platform already took: Check again hears the network say it was added. */
  "paddr-recovery-restored-finished": (t) => {
    const w = base(t);
    const lock = restoredFundLock(1, 50_000_000, "checking");
    w.pending.recovery.locks = [lock];
    w.pending.recovery.consumed = [lock.txid];
    return w;
  },
  /** A restored "Add from Dash wallet" lock, ready, while the shielded side can't be read. */
  "paddr-recovery-shielded-unread": (t) => {
    const w = base(t);
    w.pending.recovery.locks = [restoredFundLock(1, 50_000_000)];
    w.pending.recovery.unread = ["shielded"];
    return w;
  },
  /** A restored Shielded lock and a restored "Add from Dash wallet" lock, both ready. */
  "paddr-recovery-mixed": (t) => {
    const w = base(t);
    w.pending.recovery.locks = [restoredLock(1, 50_000_000), restoredFundLock(2, 25_000_000)];
    return w;
  },
  /** A transfer to Shielded of this device's that Platform refused after a penalty, 0.30 DASH left: Home offers Recover. */
  "recovery-recoverable": (t) => {
    const w = base(t);
    w.pending.recovery.locks = [partlyUsedLock(1, 50_000_000, 30_000_000_000n)];
    return w;
  },
  /** Two refused transfers to Shielded: one with 0.30 DASH left, one with less than the fee left (too small, not on the card). */
  "recovery-recoverable-small": (t) => {
    const w = base(t);
    w.pending.recovery.locks = [partlyUsedLock(1, 50_000_000, 30_000_000_000n), partlyUsedLock(2, 20_000_000, 40_000_000n)];
    return w;
  },
  /** A restored lock ready to finish and a refused transfer to Shielded with 0.30 DASH left: the sheet shows both pairs of lines. */
  "recovery-recoverable-mixed": (t) => {
    const w = base(t);
    w.pending.recovery.locks = [restoredLock(4, 25_000_000), partlyUsedLock(1, 50_000_000, 30_000_000_000n)];
    return w;
  },
  /** Only refused transfers to Shielded with less than the fee left: no card. */
  "recovery-too-small-only": (t) => {
    const w = base(t);
    w.pending.recovery.locks = [partlyUsedLock(2, 20_000_000, 40_000_000n)];
    return w;
  },
  /** A refused transfer to Shielded with 0.30 DASH left, on mainnet: listed, never offered a Recover. */
  "recovery-recoverable-mainnet": (t) => {
    const w = base(t);
    w.launch = { ...w.launch, network: { kind: "mainnet", dirName: "mainnet", unitName: "DASH" } };
    w.pending.recovery.locks = [partlyUsedLock(1, 50_000_000, 30_000_000_000n), partlyUsedLock(3, 40_000_000, 20_000_000_000n)];
    return w;
  },
  /** The first recover may have gone through ("checking"); the next is applied. */
  "recovery-recover-fails": (t) => {
    const w = base(t);
    w.pending.recovery.locks = [partlyUsedLock(1, 50_000_000, 30_000_000_000n)];
    w.pending.recovery.failures = 1;
    return w;
  },
  /** The first finish ends `platform.unavailable`; a retry is applied. */
  "recovery-finish-fails": (t) => {
    const w = base(t);
    w.pending.recovery.locks = [restoredLock(1, 50_000_000)];
    w.pending.recovery.failures = 1;
    return w;
  },
  /** A new wallet: no history, zero balance. Platform and Shielded still have their sample rows. */
  empty: (t) => base(t, "empty"),
  /** That wallet with those optional histories empty too: the commands succeed and return nothing. */
  "empty-activity": (t) => quietOptionalHistory(base(t, "empty")),
  /** That wallet while the chain is still syncing: history must not offer Receive yet. */
  "empty-syncing": (t) => {
    const w = quietOptionalHistory(base(t, "empty"));
    w.sync = syncing();
    return w;
  },
  /** No wallet on the network yet, and no vault. */
  "no-wallet": (t) => {
    const w = base(t);
    w.wallets = [];
    w.phrases = {};
    w.lock = { ...unlocked([]), state: "noVault", encrypted: false };
    return w;
  },
  /** No wallet and no vault, and a backup of two wallets on disk that Choose File… picks
   * (passphrase FIXTURE_RESTORE_BACKUP_PASSPHRASE): onboarding's Restore from a file. */
  "no-wallet-backup-file": (t) => {
    const w = base(t);
    const backedUp = [
      { walletId: FIXTURE_WALLET_ID, name: "Main", phrase: MAIN_PHRASE },
      { walletId: SECOND_WALLET_ID, name: "Savings", phrase: SECOND_PHRASE },
    ];
    w.wallets = [];
    w.phrases = {};
    w.lock = { ...unlocked([]), state: "noVault", encrypted: false };
    withRestoreBackup(w.settings, backedUp);
    w.tools.files.open = { name: "Main and Savings.dwbackup", data: "" };
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
  /** The funded wallet with links opened before it existed (DP5-01): "Links you opened" at
   * launch, a payment request, a contact and an invitation. */
  "saved-links": (t) => {
    const w = base(t);
    const now = Math.floor(Date.now() / 1000);
    stashRoute(w.stash, { kind: "invitation", linkId: "" }, now - 3 * 86_400);
    stashRoute(w.stash, { kind: "addContact", identity: SAVED_CONTACT, username: "alice" }, now - 2 * 3600);
    stashRoute(w.stash, { kind: "pay", uri: { address: SAVED_PAYEE, amount: 25_000_000, label: "Coffee", message: null }, username: null }, now - 5 * 60);
    return w;
  },
  // DP5-03: an invitation in the stash (its status `World.invitations`, else valid from Bob).
  /** The funded wallet without an identity, an invitation opened: "Links you opened" offers it. */
  "invitation-valid": (t) => withInvitation(base(t)),
  /** The same invitation, claimed by someone else. */
  "invitation-claimed": (t) => withInvitation(base(t), { kind: "claimed" }),
  /** The same invitation, not a valid one. */
  "invitation-invalid": (t) => withInvitation(base(t), { kind: "invalid", reason: "malformed" }),
  /** A vault (passphrase `demo`) with no wallet yet, an invitation opened before onboarding
   * (the welcome entry, D2). */
  "no-wallet-invitation": (t) => {
    const w = withInvitation(base(t));
    w.wallets = [];
    w.phrases = {};
    w.lock = unlocked([]);
    return w;
  },
  /** A claim of the stashed invitation stopped after a relaunch, before its claim call. */
  "invitation-claiming": (t) => {
    const w = withInvitation(base(t));
    const linkId = w.stash[0].info.id;
    const row = registering().registrations[0];
    w.dashpay = {
      ...w.dashpay,
      status: { kind: "registering", draft: "claim-1" },
      registrations: [
        { ...row, draft: "claim-1", label: "carol-77", phase: "keysPrepared", txid: null, waiting: null, holdsKey: false, running: false, fundsCommitted: false, invitation: { linkId, inviter: IDENTITY.bob, requestFailure: null } },
      ],
    };
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
  /** DP4-02: `dashpay-registered` with Alice's avatar published and verified. */
  "dashpay-avatar": (t) => withAvatar(t),
  /** DP4-03: `dashpay-avatar` with a picture for nearly everyone the lists show (contacts,
   * requests, history counterparties, the frequent strip, the bell, the search): Carol has
   * none (her initials), and Erin's thumbnail is not a PNG (the initials after it fails). */
  "dashpay-avatars": (t) => withPictures(t, true),
  /** DP4-03 D13: `dashpay-avatars` with "Load contact pictures" off: only Alice's own picture. */
  "dashpay-pictures-off": (t) => withPictures(t, false),
  /** DP4-03 D5: `dashpay-registered` with an upload host, so a picture from a file can be set. */
  "dashpay-avatar-upload": (t) => {
    const w = base(t);
    w.dashpay = { ...registered(), pictures: { upload: true, candidates: [], prepares: [] } };
    return w;
  },
  /** DP4-03: `dashpay-avatar` whose first profile save fails with `platform.unavailable`. */
  "dashpay-profile-save-error": (t) => {
    const w = withAvatar(t);
    w.dashpay.profileSaveFailures = 1;
    return w;
  },
  /** DP3-03b: `dashpay-registered` with "Include my username in payment requests" off: the
   * Receive QR and the requests are plain Core URIs. */
  "dashpay-username-off": (t) => {
    const w = base(t);
    w.dashpay = registered();
    w.wallets[0].receive.usernameInRequests = false;
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
  /** A new wallet still catching up when a 25 DASH payment arrives: the engine reports it at once,
   * InstantSend-locked and unconfirmed, in a balance flagged `catchingUp` (PRODUCT backlog 1). */
  "catching-up-payment": (t) => catchingUpPayment(t, true),
  /** The same payment seen but not InstantSend-locked yet: unconfirmed, Pending. */
  "catching-up-pending-payment": (t) => catchingUpPayment(t, false),
  /** The same new wallet before anything has arrived: Home shows 0 while it syncs. */
  "catching-up-nothing-yet": (t) => {
    const w = catchingUpPayment(t, true);
    const main = w.wallets[0];
    main.summary = { ...main.summary, balances: { ...ZERO_BALANCES, catchingUp: true } };
    main.rows = [];
    return w;
  },
  /** The first start after an update from 0.1 or 0.2: the funded wallet is replayed once from its
   * birth, so Home and Pay say why sends wait. The replay does not move on while the scenario is up. */
  "upgrade-replay": (t) => {
    const w = base(t);
    const tip = w.sync.tipHeight!;
    w.tools.rescan = { fromHeight: tip - 200_000, currentHeight: tip - 150_000, targetHeight: tip, startedAt: secs(FIXTURE_NOW), upgradeWallets: [w.wallets[0].summary.walletId] };
    w.tools.rescanStep = 0;
    return w;
  },
  /** The wallet's history starts before the block data this install keeps: Home says its older
   * payments aren't shown and offers Repair's reset, which clears it. */
  "history-gap": (t) => withHistoryGap(base(t)),
  /** The notice with mixing stopped but a signed round's inputs still held: its reset is refused. */
  "history-gap-settling": (t) => {
    const w = withHistoryGap(base(t));
    w.tools.inputsHeld = true;
    return w;
  },
  /** The same for two wallets. */
  "history-gap-two-wallets": (t) => withHistoryGap(twoWallets(base(t))),
  /** Past its birth block (`catchingUp` false) but 2,000 blocks short of the tip, SPV stopped. */
  "wallet-behind-stopped": (t) => walletBehind(t, true, false),
  /** The same with nothing found yet: Home shows 0. */
  "wallet-behind-stopped-empty": (t) => walletBehind(t, true, true),
  /** The network caught up while this wallet's own scan is still 2,000 blocks short. */
  "wallet-behind-at-tip": (t) => walletBehind(t, false, false),
  "wallet-behind-at-tip-empty": (t) => walletBehind(t, false, true),
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
  /** The same while the wallet's Platform-address pass failed: the bar says Platform did not finish. */
  "dashpay-identity-unsettled-addresses-failed": (t) => {
    const w = withStatus(base(t), { kind: "startupIncomplete", startup: "identityUnsettled" });
    w.lock = { ...w.lock, state: "unlockedMixingOnly" };
    w.dashpay.addresses = "failed";
    return w;
  },
  /** The bring-up could not finish its discovery. */
  "dashpay-look-failed": (t) => withStatus(base(t), { kind: "startupIncomplete", startup: "discoveryFailed" }),
  // DP6-03: Tools ▸ Information's DashPay card. `dashpay-registered` is its degraded mode (the
  // default quorum source, DEC-168); the states without an identity are the scenarios above.
  /** Quorum keys from SPV, a pass that found two requests and a contact, two contact accounts
   * waiting to be built: the card says Running. Forward-looking: the engine reports no pass yet
   * (`last_pass` is `None`, DP6-03 D4/F2), so the Last pass row is ahead of today's engine. */
  "dashpay-info-spv": (t) => {
    const w = withSyncInfo(t, { source: "spv", trustedOnly: false }, pass({ newRequests: 2, newContacts: 1 }));
    w.dashpay.startup = { ...w.dashpay.startup, contactAccountsPending: 2 };
    return w;
  },
  /** Before masternode sync: the trusted service's keys, results unverified. */
  "dashpay-info-fallback": (t) => withSyncInfo(t, { source: "trustedFallback", trustedOnly: false }),
  /** The developer toggle: the trusted service only. */
  "dashpay-info-trusted-only": (t) => withSyncInfo(t, { source: "trusted", trustedOnly: true }),
  /** The last pass failed with `platform.unavailable`. Forward-looking, as `dashpay-info-spv`'s
   * pass: no engine reports one yet (DP6-03 D4/F2). */
  "dashpay-info-pass-failed": (t) => withSyncInfo(t, { source: "spv", trustedOnly: false }, pass({ failureCode: "platform.unavailable" })),
  /** No identity, which Platform proved within its 7 days: the bring-up did not run (bringup.rs),
   * and the card still says Not set up. */
  "dashpay-info-no-identity-proven": (t) => {
    const w = base(t);
    w.dashpay.startup = { ...w.dashpay.startup, startup: "notRun", finishedAt: null };
    return w;
  },
  /** An identity on file whose bring-up runs, before SPV and the loops start (every launch,
   * bringup.rs `supervise`): Starting, not Stopped (DP6-03 D19). */
  "dashpay-info-bringup-running": (t) => {
    const w = withSyncInfo(t, { source: "spv", trustedOnly: false });
    w.dashpay.startup = { ...w.dashpay.startup, startup: "starting", finishedAt: null };
    w.sync = { ...w.sync, running: false, connectedPeers: 0 };
    return w;
  },
  /** An identity on file whose bring-up ended with contact accounts still to build (the library's
   * `PartialAccountsPending`, the common case for them): the identity settles it, Running. */
  "dashpay-info-identity-incomplete": (t) => {
    const w = withSyncInfo(t, { source: "spv", trustedOnly: false });
    w.dashpay.startup = { ...w.dashpay.startup, startup: "partialAccountsPending", contactAccountsPending: 3 };
    return w;
  },
  /** SPV stopped: the loops do not run, so Sync now has nothing to run. */
  "dashpay-info-stopped": (t) => {
    const w = withSyncInfo(t, { source: "spv", trustedOnly: false });
    w.sync = { ...w.sync, running: false, connectedPeers: 0 };
    return w;
  },
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
    w.dashpay.registrations[0] = { ...w.dashpay.registrations[0], phase: "draft", waiting: "unlock", txid: null, holdsKey: false, running: false, fundsCommitted: false };
    w.dashpay.runs = [];
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
      running: false,
      fundsCommitted: false,
      failure: { phase: "keysPrepared", code: "registration.funding_insufficient", retryable: true, needed: "3002260", available: "1000000" },
    };
    w.dashpay.runs = [];
    return w;
  },
  /** Paid from the Platform balance, and Platform has not said whether the payment went through:
   * Continue pays it again, Move to Dash wallet frees the balance (F22). */
  "dashpay-registering-held": (t) => {
    const w = base(t);
    w.dashpay = registering();
    w.dashpay.registrations[0] = {
      ...w.dashpay.registrations[0],
      phase: "keysPrepared",
      waiting: "paymentHeld",
      txid: null,
      holdsKey: false,
      running: false,
      fundsCommitted: true,
      fromPlatform: true,
    };
    w.dashpay.runs = [];
    return w;
  },
  /** Paid from the Shielded balance, its outcome unknown, and the app reopened since: nothing
   * checks it, so Continue checks it again (F12). */
  "dashpay-registering-shielded-recheck": (t) => {
    const w = base(t);
    w.dashpay = registering();
    w.dashpay.registrations[0] = {
      ...w.dashpay.registrations[0],
      phase: "keysPrepared",
      waiting: "paymentUnknown",
      txid: null,
      holdsKey: false,
      running: false,
      fundsCommitted: true,
      fromShielded: true,
    };
    w.dashpay.runs = [];
    return w;
  },
  // DP1-09: a registration's screens (R2, R5, R8). Its run ended with the last launch.
  /** Funded, after a relaunch: no proof yet, so it reads `chainLock` with no key held, and waits for Continue (R2, D21). */
  "dashpay-registration-paused": (t) => {
    const w = base(t);
    w.dashpay = { ...registering(), runs: [] };
    return w;
  },
  /** Its asset lock is proved but no flow finished it: Finish transfers (R5). */
  "dashpay-registration-needs-finish": (t) => {
    const w = base(t);
    w.dashpay = { ...registering(), runs: [] };
    w.dashpay.registrations[0].waiting = "needsFinish";
    return w;
  },
  /** Failed for good after its funding went out: the recovery copy and Repair (R8). The status no
   * longer names it (banner.rs `in_progress`); the Join page finds it (D22). */
  "dashpay-registration-stopped": (t) => {
    const w = base(t);
    w.dashpay = { ...registering(), runs: [], status: { kind: "noIdentity", reason: null } };
    w.dashpay.registrations[0] = {
      ...w.dashpay.registrations[0],
      phase: "failed",
      waiting: null,
      holdsKey: false,
      running: false,
      failure: { phase: "fundingSent", code: "registration.islock_timeout", retryable: false, needed: null, available: null },
    };
    return w;
  },
  /** Failed (a retryable timeout) while its funding's outcome is unknown: the engine keeps the
   * wait of the step it still owns (`Awaiting`), so the page, Home and Tools ▸ Repair send it to
   * Resolve stuck funding (f1, Sol r1 P2). */
  "dashpay-registration-failed-unknown": (t) => {
    const w = base(t);
    w.dashpay = { ...registering(), runs: [] };
    w.dashpay.registrations[0] = {
      ...w.dashpay.registrations[0],
      phase: "failed",
      waiting: "fundingUnknown",
      holdsKey: false,
      running: false,
      fundsCommitted: true,
      failure: { phase: "keysPrepared", code: "platform.timeout", retryable: true, needed: null, available: null },
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
  /** DP6-01: Alice and her work identity, the main one by choice; keys for both; a third identity
   * of the seed for Find my identities. */
  "dashpay-identities": (t) => {
    const w = base(t);
    w.dashpay = identitiesWorld();
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
  /** The funded wallet watch-only: Pay opens Advanced: unsigned transaction (PSBT), Review makes a
   * PSBT, and the confirm sheet offers Create Unsigned only (P15, U-05 D18). */
  "watch-only": (t) => {
    const w = base(t);
    w.wallets[0].summary = { ...w.wallets[0].summary, watchOnly: true };
    return w;
  },
  /** Coin Selection: CoinJoin denominations beside the plain coins, hidden by default (QT-071). */
  "tools-coinjoin": (t) => withCoinJoinCoins(base(t)),
  /** Every coin is mixed, so Coin Selection's default list is empty until Show all coins. */
  "coins-all-mixed": (t) => {
    const w = base(t);
    w.wallets[0].coins = w.wallets[0].coins.map((c) => ({ ...c, coinjoinDenominated: true, coinjoinRounds: 4 }));
    return w;
  },
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
  /** H-09: a payment to Erin, whose data is unverified, waits in `prepare` and is refused with
   * `platform.context_unavailable` (`hold=draft_prepare` shows the wait). */
  "dashpay-contact-unchecked": (t) => {
    const w = base(t);
    w.dashpay = { ...registered(), unverifiedPrepare: "contextUnavailable" };
    return w;
  },
  /** H-09: a payment to Erin is refused with `platform.trust_mismatch`. */
  "dashpay-contact-mismatch": (t) => {
    const w = base(t);
    w.dashpay = { ...registered(), unverifiedPrepare: "trustMismatch" };
    return w;
  },
  /** H-09: Grace's request comes from a profile not yet verified: Accept warns and stays on. */
  "dashpay-unverified-request": (t) => {
    const w = base(t);
    w.dashpay = registered();
    w.dashpay.contacts = w.dashpay.contacts.map((c) => (c.summary.username === "grace" ? { ...c, summary: { ...c.summary, unverified: true } } : c));
    return w;
  },
  /** "alice" with fewer credits than one contact request: Accept is refused (DP2-09 D4). */
  "dashpay-low-credits": (t) => {
    const w = base(t);
    w.dashpay = registered();
    w.dashpay.identities = w.dashpay.identities.map((i) => ({ ...i, balance: "4000000", creditLevel: "empty" }));
    return w;
  },
  // DP1-10: My Profile's credits block, top-ups and the request card.
  /** "alice" at or below the low threshold: the low warning and Home's card (C2, S19). */
  "dashpay-credits-low": (t) => withAlice(base(t), { balance: "1000000000", creditLevel: "low" }),
  /** The balance not read yet (C4). */
  "dashpay-credits-unknown": (t) => withAlice(base(t), { balance: null, balanceAsOf: null, balanceStale: true, creditLevel: null }),
  /** The balance may be behind (C5). */
  "dashpay-credits-stale": (t) => withAlice(base(t), { balanceStale: true }),
  /** A top-up of 0.05 Dash waiting for its proof (C6). */
  "dashpay-top-up-pending": (t) => withCredits(base(t), { pending: [topUpLock({ proved: false })] }),
  /** A top-up lock Platform answered "already consumed" for, proved (C6). */
  "dashpay-top-up-unknown": (t) => withCredits(base(t), { pending: [topUpLock({ proved: true, consumptionUnknown: true })] }),
  /** A top-up funding whose outcome is unknown: Repair resolves it (C7, D16). */
  "dashpay-top-up-stuck": (t) =>
    withCredits(base(t), { stuck: [{ step: topUpStep(0, 1), identityIndex: 0, identity: IDENTITY.alice, state: "maybeSent" }] }),
  /** The next top-up's funding outcome is unknown: the sheet routes to Repair (T8, D16, D22). */
  "dashpay-top-up-lost": (t) => withCredits(base(t), { next: "broadcastUnknown" }),
  // DP6-02: withdrawals. Refusals the fixture does not model come from `&fail=credits_withdraw:<class>`.
  /** "alice" with a withdrawal in each state My Profile shows (D11). */
  "dashpay-withdrawals": (t) =>
    withCredits(base(t), {
      withdrawals: [
        withdrawal(4, "queued", 50_000_000_000, 1),
        withdrawal(3, "expired", 20_000_000_000, 5),
        withdrawal(2, "complete", 100_000_000_000, 26),
        withdrawal(1, "failed", 1_190_000, 50),
      ],
    }),
  /** The next withdrawal's broadcast goes unanswered and its document is not found (§3). */
  "dashpay-withdraw-lost": (t) => withCredits(base(t), { nextWithdraw: "unknown" }),
  /** The next withdrawal's broadcast goes unanswered but its document is read back (§3). */
  "dashpay-withdraw-landed": (t) => withCredits(base(t), { nextWithdraw: "landed" }),
  /** The next withdrawal goes unanswered and unlisted, then executes before the one after it,
   * which is refused naming it (D24). */
  "dashpay-withdraw-lost-landed": (t) => withCredits(base(t), { nextWithdraw: "lostLanded" }),
  /** "alice" contested with no temporary name: Create instant username (R1, D13). */
  "dashpay-contest-no-temporary": (t) => {
    const w = contestPending(base(t), 1_900_000_000);
    w.dashpay.identities = w.dashpay.identities.map((i) => ({ ...i, names: [], mainName: null }));
    return w;
  },
  /** "alice" contested with no temporary name and too few credits for one: Top up (D4 reversed). */
  "dashpay-contest-instant-short": (t) => {
    const w = contestPending(base(t), 1_900_000_000);
    w.dashpay.identities = w.dashpay.identities.map((i) => ({ ...i, names: [], mainName: null, balance: "1000000", creditLevel: "low" }));
    return w;
  },
  /** DP1-09 D4 reversed: the registration's temporary "alice2" registered beside the contested
   * "alice" (iOS's success line), or refused (its failure line and Try again). */
  "dashpay-instant-ok": (t) => withInstant(base(t), true),
  "dashpay-instant-failed": (t) => withInstant(base(t), false),
  /** The vote on "alice" won (the main name switches, "alice2" kept) or lost ("alice2" stays main). */
  "dashpay-instant-won": (t) => withContest(withInstant(base(t), true), { kind: "won" }, "alice2"),
  "dashpay-instant-lost": (t) => withContest(withInstant(base(t), true), { kind: "lost", winner: IDENTITY.mallory }, "alice2"),
  /** The contest read before the status moved on: won (R3), lost to another user (R4), blocked (R5). */
  "dashpay-contest-won": (t) => withContest(contestPending(base(t), 1_900_000_000), { kind: "won" }),
  "dashpay-contest-lost": (t) => withContest(contestPending(base(t), 1_900_000_000), { kind: "lost", winner: IDENTITY.mallory }),
  "dashpay-contest-locked": (t) => withContest(contestPending(base(t), 1_900_000_000), { kind: "locked" }),
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
  // CoinJoin (U-06): coinjoin.ts.
  ...coinjoinScenarios(base),
  // Governance (MG-09): governance.ts.
  ...governanceScenarios(base),
  // My masternodes (MG-10): masternodes.ts.
  ...masternodesScenarios(base),
  // Shared masternodes (MG-10b): shared.ts.
  ...sharedScenarios(base),
  // Prices in fiat (MP-01): rates.ts.
  ...ratesScenarios(base),
  // Transfer (POLISH 25): transfer.ts.
  ...transferScenarios(base),
  /** A new install on Mainnet: onboarding's last step carries the price disclosure. */
  "fiat-first-run": (t): World => onMainnet(buildWorld("no-wallet", t)),
  /** `dashpay-registered` with prices: paying a contact with the fiat toggle. */
  "fiat-contacts": (t): World => withPrices(buildWorld("dashpay-registered", t)),
  /** Platform protocol version not learned yet (`null`). */
  "protocol-unknown": (t): World => ({ ...base(t, "default", true), protocolCompatibility: { networkVersion: null, supportedVersion: 15 } }),
  /** Platform protocol version equal to this build's supported version (`15`). */
  "protocol-equal": (t): World => ({ ...base(t, "default", true), protocolCompatibility: { networkVersion: 15, supportedVersion: 15 } }),
  /** Platform runs a newer protocol version (`16` > `15`): Home shows the advisory banner. */
  "protocol-upgraded": (t): World => ({ ...base(t, "default", true), protocolCompatibility: { networkVersion: 16, supportedVersion: 15 } }),
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
