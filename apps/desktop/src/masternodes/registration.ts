// The registration wizard's model (MG-10 §1.6, D5): what the user picked, the request it builds,
// the field checks made before any call, the register grant's cap, and where a refusal shows.

import { isAppError, type Backend } from "../api/backend";
import type { AppError } from "../api/generated/commands";
import type { CollateralCoin } from "../api/generated/CollateralCoin";
import type { MnRegistrationPreview } from "../api/generated/MnRegistrationPreview";
import type { MnRegistrationRequest } from "../api/generated/MnRegistrationRequest";
import type { MasternodeTypeName } from "../api/generated/MasternodeTypeName";
import type { ProTxKeys } from "../api/generated/ProTxKeys";
import { t } from "../i18n";
import { DUP_NODE, DUP_SERVICE, PAYEE_REUSE } from "./protxReasons";

const P = "masternodes.register";

export const STEPS = ["type", "collateral", "operator", "keys", "review"] as const;
export type Step = (typeof STEPS)[number] | "prove" | "done";

export type Runner = "hosted" | "self";

/** A registration may be prepared and sent on testnet, regtest and devnet. Mainnet stays off until
 * a registration's locked coins can always come back (D14). Still loading: off. */
export const registrationSends = (network: { kind: string } | null | undefined): boolean =>
  network != null && network.kind !== "mainnet";

/** The calls a held wizard may make: they read, and lock no coin. */
export const HELD_CALLS = ["mnRegistrationKeys", "mnPreviewRegistration"] as const;
/** The calls that lock, spend or release a coin, refused while held. */
export const SENDING_CALLS = [
  "mnRegister",
  "mnPrepareExternalRegistration",
  "mnRefreshExternalRegistration",
  "mnSubmitExternalRegistration",
  "mnResendOperation",
  "mnReleaseCollateral",
  "mnLockCollateral",
] as const;
export type RegistrationCalls = Pick<Backend, (typeof HELD_CALLS)[number] | (typeof SENDING_CALLS)[number]>;

/** The backend as the wizard, its rows and the collateral actions see it: while held, every
 * sending call rejects before it reaches the backend. */
export function registrationCalls(b: Backend, sends: boolean): RegistrationCalls {
  const calls = Object.fromEntries(HELD_CALLS.map((k) => [k, b[k].bind(b)])) as Pick<Backend, (typeof HELD_CALLS)[number]>;
  const held = () => Promise.reject(new Error("registration sending is held"));
  const sending = Object.fromEntries(SENDING_CALLS.map((k) => [k, sends ? b[k].bind(b) : held]));
  return { ...calls, ...sending } as RegistrationCalls;
}

/** The step's main button: Continue, Register (Review), Register (Prove), or, while held, none at
 * Review and the send-off line in its place. */
export type FinalAction = "continue" | "register" | "submit" | "sendOff";

export function finalAction(step: Step, external: boolean, sends: boolean): FinalAction {
  if (step === "prove") return sends ? "submit" : "sendOff";
  if (step === "review") return !sends ? "sendOff" : external ? "continue" : "register";
  return "continue";
}
export type CollateralPick = "fund_new" | "existing" | "external";

/** Review's line under the collateral: this wallet locks its own coin; another wallet's must stay unspent there. */
export const collateralNote = (pick: CollateralPick) => (pick === "external" ? `${P}.review.collateralKeepUnspent` : `${P}.review.collateralLocked`);

/** Everything the wizard asks, as typed. */
export interface RegDraft {
  kind: MasternodeTypeName;
  runner: Runner;
  collateral: CollateralPick;
  /** ExistingUtxo: the coin picked. */
  outpoint: string;
  /** External: the output and its address, as typed. */
  externalOutpoint: string;
  externalAddress: string;
  service: string;
  /** Hosted, or self-run with "Enter one instead": the operator's BLS public key. */
  operatorKey: string;
  operatorFromWallet: boolean;
  /** The operator's share in percent, as typed ("5", "2.5"). */
  operatorShare: string;
  nodeFromWallet: boolean;
  nodeId: string;
  p2pPort: string;
  httpPort: string;
  voting: "wallet" | "address";
  votingAddress: string;
  payout: "new" | "address";
  payoutAddress: string;
}

export const newDraft = (): RegDraft => ({
  kind: "regular",
  runner: "hosted",
  collateral: "fund_new",
  outpoint: "",
  externalOutpoint: "",
  externalAddress: "",
  service: "",
  operatorKey: "",
  operatorFromWallet: true,
  operatorShare: "0",
  nodeFromWallet: true,
  nodeId: "",
  p2pPort: "",
  httpPort: "",
  voting: "wallet",
  votingAddress: "",
  payout: "new",
  payoutAddress: "",
});

/** The collateral, in Dash: 1,000 or 4,000 (an evonode). */
export const collateralDash = (kind: MasternodeTypeName) => (kind === "evonode" ? 4000 : 1000);

const COIN = 100_000_000;

/** The coins of `kind`'s collateral amount, free first (ExistingUtxo's list, §1.6 step 2). */
export const coinsFor = (coins: CollateralCoin[], kind: MasternodeTypeName) =>
  coins.filter((c) => c.kind === kind && c.amount === collateralDash(kind) * COIN).sort((a, b) => Number(a.userLocked) - Number(b.userLocked));

/** Self-run takes the operator key from the keychain unless the user enters one. */
const walletOperator = (d: RegDraft) => d.runner === "self" && d.operatorFromWallet;
const walletNode = (d: RegDraft) => d.runner === "self" && d.nodeFromWallet;

/** Basis points from a percent with at most two decimals, 0–100; `null` when it isn't one. */
export function shareBasisPoints(text: string): number | null {
  const s = text.trim() === "" ? "0" : text.trim();
  if (!/^\d{1,3}(\.\d{1,2})?$/.test(s)) return null;
  const bp = Math.round(Number(s) * 100);
  return bp <= 10_000 ? bp : null;
}

/** dash-qt writes `txid:n`; the engine reads `txid-n`. */
export const normalOutpoint = (s: string) => s.trim().replace(/:(\d+)$/, "-$1").toLowerCase();

const port = (s: string) => (s.trim() === "" ? null : Number(s.trim()));

/** The engine's request for what the wizard holds; `keys` gives the keychain indices shown. */
export function buildRequest(d: RegDraft, keys: ProTxKeys | undefined): MnRegistrationRequest {
  const evo = d.kind === "evonode";
  return {
    kind: d.kind,
    collateral:
      d.collateral === "fund_new"
        ? { kind: "fund_new" }
        : d.collateral === "existing"
          ? { kind: "existing_utxo", outpoint: d.outpoint }
          : { kind: "external", outpoint: normalOutpoint(d.externalOutpoint), address: d.externalAddress.trim() },
    service: d.service.trim(),
    operator: walletOperator(d) ? (keys ? { kind: "keychain", index: keys.operator.index } : { kind: "next_unused" }) : { kind: "given", publicKey: d.operatorKey.trim() },
    operatorReward: shareBasisPoints(d.operatorShare) ?? 0,
    ownerIndex: keys?.owner.index ?? null,
    voting: d.voting === "address" ? { kind: "address", address: d.votingAddress.trim() } : keys ? { kind: "keychain", index: keys.voting.index } : { kind: "next_unused" },
    payout: d.payout === "address" ? { kind: "address", address: d.payoutAddress.trim() } : { kind: "new_address" },
    evonode: evo
      ? {
          node: walletNode(d) ? (keys?.platformNode ? { kind: "keychain", index: keys.platformNode.index } : { kind: "next_unused" }) : { kind: "given", nodeId: d.nodeId.trim() },
          p2pPort: port(d.p2pPort),
          httpPort: port(d.httpPort),
        }
      : null,
  };
}

// ---------------------------------------------------------------- field checks before any call

export type Field = "outpoint" | "externalOutpoint" | "externalAddress" | "service" | "operatorKey" | "operatorShare" | "nodeId" | "p2pPort" | "httpPort" | "votingAddress" | "payoutAddress" | "signature";
export type FieldErrors = Partial<Record<Field, string>>;

const IPV4_PORT = /^(\d{1,3})\.(\d{1,3})\.(\d{1,3})\.(\d{1,3}):(\d{1,5})$/;
const OUTPOINT = /^[0-9a-fA-F]{64}[:-]\d{1,10}$/;
const BLS_HEX = /^[0-9a-fA-F]{96}$/;
const NODE_ID = /^[0-9a-fA-F]{40}$/;

/** The network's masternode port, and the other network's, for the field hint. */
export interface Ports {
  own: number;
  /** A port that belongs to another network, and its name. */
  other: { port: number; network: string } | null;
  network: string;
}

/** Step 3's server address (G3: IPv4 only, the wallet's own forms). */
export function serviceError(service: string, ports: Ports): string | null {
  const s = service.trim();
  if (s === "") return t(`${P}.error.serviceMissing`);
  const m = IPV4_PORT.exec(s);
  if (!m || m.slice(1, 5).some((o) => Number(o) > 255)) return t(`${P}.error.serviceIpv4`);
  const p = Number(m[5]);
  if (p < 1 || p > 65535) return t(`${P}.error.serviceIpv4`);
  if (ports.other && p === ports.other.port) return t(`${P}.error.servicePort`, { port: String(p), otherNetwork: ports.other.network, network: ports.network, own: String(ports.own) });
  return null;
}

function portError(s: string): string | null {
  if (s.trim() === "") return null;
  const n = Number(s.trim());
  return Number.isInteger(n) && n >= 1 && n <= 65535 ? null : t(`${P}.error.port`);
}

/** The errors a step's fields have before Continue may call anything. */
export function stepErrors(step: Step, d: RegDraft, ports: Ports, coins: readonly CollateralCoin[], ownerAddress: string | null): FieldErrors {
  const e: FieldErrors = {};
  if (step === "collateral") {
    if (d.collateral === "existing" && d.outpoint === "") e.outpoint = t(`${P}.error.pickCoin`);
    if (d.collateral === "external") {
      if (!OUTPOINT.test(d.externalOutpoint.trim())) e.externalOutpoint = t(`${P}.error.outpoint`);
      else if (coins.some((c) => c.outpoint.toLowerCase() === normalOutpoint(d.externalOutpoint))) e.externalOutpoint = t(`${P}.error.collateralInWallet`, { amount: collateralDash(d.kind) });
      if (d.externalAddress.trim() === "") e.externalAddress = t(`${P}.error.address`);
    }
  }
  if (step === "operator") {
    const service = serviceError(d.service, ports);
    if (service) e.service = service;
    if (!walletOperator(d) && !BLS_HEX.test(d.operatorKey.trim())) e.operatorKey = t(`${P}.error.operatorKey`);
    if (shareBasisPoints(d.operatorShare) === null) e.operatorShare = t(`${P}.error.share`);
    if (d.kind === "evonode") {
      if (!walletNode(d) && !NODE_ID.test(d.nodeId.trim())) e.nodeId = t(`${P}.error.nodeId`);
      const p2p = portError(d.p2pPort);
      const http = portError(d.httpPort);
      if (p2p) e.p2pPort = p2p;
      if (http) e.httpPort = http;
      if (!p2p && !http && d.p2pPort.trim() !== "" && d.p2pPort.trim() === d.httpPort.trim()) e.httpPort = t(`${P}.error.samePorts`);
    }
  }
  if (step === "keys") {
    if (d.voting === "address") {
      if (d.votingAddress.trim() === "") e.votingAddress = t(`${P}.error.address`);
      else if (ownerAddress !== null && d.votingAddress.trim() === ownerAddress) e.votingAddress = t(`${P}.error.votingIsOwner`);
    }
    if (d.payout === "address" && d.payoutAddress.trim() === "") e.payoutAddress = t(`${P}.error.address`);
  }
  return e;
}

// ---------------------------------------------------------------- the register grant (D5)

/** MG-09 D29's margin: the engine picks the coins again at register time. */
const REGISTER_MARGIN = 10_000;

/** D5: the confirmed `total_debit` plus max(fee, 10,000 duffs). Never from a figure not shown. */
export const registerCap = (p: Pick<MnRegistrationPreview, "fee" | "totalDebit">): number => p.totalDebit + Math.max(p.fee, REGISTER_MARGIN);

/** Whether the re-read preview shows what the review showed (D5, D4). */
export const samePreview = (a: MnRegistrationPreview, b: MnRegistrationPreview): boolean =>
  a.fee === b.fee && a.totalDebit === b.totalDebit && a.collateralAmount === b.collateralAmount && JSON.stringify(a.terms) === JSON.stringify(b.terms);

// ---------------------------------------------------------------- refusals, by the step they belong to

export interface Refusal {
  step: Step;
  field: Field | null;
  text: string;
}

const ROLES = ["owner", "voting", "operator", "platform_node"] as const;
const role = (e: AppError) => ROLES.find((r) => e.data?.kind === "keyRole" && e.data.role === r) ?? "owner";

/** Where a registration refusal shows (§1.6): the step and field it belongs to, or `null` to show it on the current step. */
export function refusalOf(e: unknown, kind: MasternodeTypeName = "regular"): Refusal | null {
  if (!isAppError(e)) return null;
  const reason = e.data?.kind === "reason" ? e.data.reason : null;
  switch (e.code) {
    case "masternodes.protx_refused":
      if (reason === DUP_SERVICE) return { step: "operator", field: "service", text: t(`${P}.refused.dupService`) };
      if (reason === "bad-protx-dup-key") return { step: "operator", field: "operatorKey", text: t(`${P}.refused.dupKey`) };
      if (reason === DUP_NODE) return { step: "operator", field: "nodeId", text: t(`${P}.refused.dupNode`) };
      if (reason === "protx-dup") return { step: "operator", field: null, text: t(`${P}.refused.pendingDup`) };
      if (reason === PAYEE_REUSE) return { step: "keys", field: "payoutAddress", text: t(`${P}.refused.payeeReuse`) };
      if (reason === "bad-protx-operator-reward") return { step: "operator", field: "operatorShare", text: t(`${P}.error.share`) };
      if (reason?.startsWith("bad-protx-netinfo")) return { step: "operator", field: "service", text: t(`${P}.error.serviceIpv4`) };
      if (reason?.startsWith("bad-protx-collateral")) return { step: "collateral", field: null, text: t(`${P}.refused.collateral`, { amount: collateralDash(kind) }) };
      return { step: "review", field: null, text: t(`${P}.refused.rules`, { reason: reason ?? e.message }) };
    case "masternodes.keys_exhausted": {
      const r = role(e);
      const operatorSide = r === "operator" || r === "platform_node";
      return { step: operatorSide ? "operator" : "keys", field: null, text: t(operatorSide ? `${P}.refused.exhausted100` : `${P}.refused.exhausted20`, { role: t(`${P}.role.${r}`) }) };
    }
    case "masternodes.collateral_unavailable":
      return { step: "collateral", field: "outpoint", text: t(`${P}.refused.collateralUnavailable`) };
    case "invalid_argument":
      if (e.message.includes("collateral is this wallet's")) return { step: "collateral", field: "externalAddress", text: t(`${P}.error.collateralInWallet`, { amount: collateralDash(kind) }) };
      return null;
    case "masternodes.signature_mismatch":
      return { step: "prove", field: "signature", text: t(`${P}.refused.signatureMismatch`) };
    default:
      return null;
  }
}
