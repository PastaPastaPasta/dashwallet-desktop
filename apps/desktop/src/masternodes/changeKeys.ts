// Change keys or payout (MG-10 §1.7, MG-00 §4.25.6): the form's request, its checks, the review
// lines and the refusals. A ProUpRegTx is signed with the owner key only; the operator key is a
// public key the operator gives, or this wallet's keychain key, never a secret.

import { isAppError } from "../api/backend";
import type { MasternodeDetail } from "../api/generated/MasternodeDetail";
import type { ProTxOperation } from "../api/generated/ProTxOperation";
import type { UpdateRegistrarPreview } from "../api/generated/UpdateRegistrarPreview";
import type { UpdateRegistrarRequest } from "../api/generated/UpdateRegistrarRequest";
import { t } from "../i18n";
import { mnErrorText } from "./model";
import { DUP_KEY, PAYEE_REUSE, PENDING_DUP } from "./protxReasons";

const P = "masternodes.myMasternodes.changeKeys";
const BLS_HEX = /^[0-9a-fA-F]{96}$/;
const ADDRESS = /^[1-9A-HJ-NP-Za-km-z]{26,35}$/;

export type OperatorPick = "keep" | "keychain" | "given";
export type VotingPick = "keep" | "keychain" | "address";
export type PayoutPick = "recorded" | "new" | "address";

export interface ChangeKeysForm {
  operator: OperatorPick;
  operatorKey: string;
  voting: VotingPick;
  votingAddress: string;
  payout: PayoutPick | null;
  payoutAddress: string;
}

/** The payout address in this wallet's records, which Q4 preselects. */
export const recordedPayout = (d: MasternodeDetail): string | null => d.registration?.payoutAddress ?? null;

/** Every key kept; the recorded payout selected for the first preview, which keeps it only when it reads
 * `payoutChanges: false` (MG-10 Q4, MG-04 D10; see `keepsRecordedPayout`). */
export function initialForm(d: MasternodeDetail): ChangeKeysForm {
  return { operator: "keep", operatorKey: "", voting: "keep", votingAddress: "", payout: recordedPayout(d) ? "recorded" : null, payoutAddress: "" };
}

export interface FieldErrors {
  operatorKey?: string;
  votingAddress?: string;
  payoutAddress?: string;
}

/** The field errors of what was typed; an empty field is incomplete, not wrong. */
export function fieldErrors(f: ChangeKeysForm): FieldErrors {
  const e: FieldErrors = {};
  const key = f.operatorKey.trim();
  if (f.operator === "given" && key && !BLS_HEX.test(key)) e.operatorKey = t(`${P}.operator.givenInvalid`);
  const voting = f.votingAddress.trim();
  if (f.voting === "address" && voting && !ADDRESS.test(voting)) e.votingAddress = t(`${P}.voting.addressInvalid`);
  const payout = f.payoutAddress.trim();
  if (f.payout === "address" && payout && !ADDRESS.test(payout)) e.payoutAddress = t(`${P}.payout.addressInvalid`);
  return e;
}

/** The request, or `null` while the form is incomplete or a field is wrong. */
export function buildRequest(proTxHash: string, f: ChangeKeysForm, recorded: string | null): UpdateRegistrarRequest | null {
  if (Object.keys(fieldErrors(f)).length > 0) return null;
  const key = f.operatorKey.trim();
  const voting = f.votingAddress.trim();
  const payout = f.payoutAddress.trim();
  if (f.operator === "given" && !key) return null;
  if (f.voting === "address" && !voting) return null;
  let payoutChoice: UpdateRegistrarRequest["payout"];
  if (f.payout === "recorded" && recorded) payoutChoice = { kind: "address", address: recorded };
  else if (f.payout === "new") payoutChoice = { kind: "new_address" };
  else if (f.payout === "address" && payout) payoutChoice = { kind: "address", address: payout };
  else return null;
  return {
    proTxHash,
    operator: f.operator === "keep" ? { kind: "keep" } : f.operator === "keychain" ? { kind: "next_unused" } : { kind: "given", publicKey: key.toLowerCase() },
    voting: f.voting === "keep" ? { kind: "keep" } : f.voting === "keychain" ? { kind: "next_unused" } : { kind: "address", address: voting },
    payout: payoutChoice,
  };
}

/** Q4: the recorded payout stays preselected only while the engine reads it as no change. */
export const keepsRecordedPayout = (p: UpdateRegistrarPreview) => p.payoutChanges === false;

/** What Send signs: the reviewed request with a "next unused" key pinned to the index the preview showed, so
 * the signed key is the reviewed one or the engine refuses it. */
export function pinToPreview(r: UpdateRegistrarRequest, p: UpdateRegistrarPreview): UpdateRegistrarRequest {
  const operator = r.operator.kind === "next_unused" && p.terms.operatorIndex !== null ? { kind: "keychain" as const, index: p.terms.operatorIndex } : r.operator;
  const voting = r.voting.kind === "next_unused" && p.terms.votingIndex !== null ? { kind: "keychain" as const, index: p.terms.votingIndex } : r.voting;
  return { ...r, operator, voting };
}

/** A registrar update may be sent on testnet, regtest and devnet. Mainnet stays off until a sent
 * update whose transaction never confirms releases its fee coins. Still loading: off. */
export const registrarUpdateSends = (network: { kind: string } | null | undefined): boolean =>
  network != null && network.kind !== "mainnet";

/** A registrar update of this masternode that is signed but didn't go out: resend or discard it. */
export const unsentUpdate = (ops: ProTxOperation[] | undefined, proTxHash: string) =>
  ops?.find((o) => o.kind === "update_registrar" && o.proTxHash === proTxHash && (o.state.kind === "not_sent" || o.state.kind === "signed")) ?? null;

/** The grant was cut from a fee that has since gone up. */
export const feeWentUp = (e: unknown) => isAppError(e) && e.code === "send.grant_exceeded";

/** The preview changes nothing: the send would only pay a fee. */
export const changesNothing = (p: UpdateRegistrarPreview) => !p.operatorChanges && !p.votingChanges && p.payoutChanges === false;

export interface ReviewLine {
  id: "operator" | "voting" | "payout";
  title: string;
  value: string;
  status: string;
  changes: boolean;
}

const short = (s: string) => (s.length > 20 ? `${s.slice(0, 10)}…${s.slice(-6)}` : s);

/** What the masternode will read after the update, and whether each part changes. */
export function reviewLines(p: UpdateRegistrarPreview): ReviewLine[] {
  const status = (changes: boolean | null) => t(changes === null ? `${P}.review.set` : changes ? `${P}.review.changes` : `${P}.review.stays`);
  return [
    { id: "operator", title: t(`${P}.review.operator`), value: short(p.terms.operatorPublicKey), status: status(p.operatorChanges), changes: p.operatorChanges },
    { id: "voting", title: t(`${P}.review.voting`), value: short(p.terms.votingAddress), status: status(p.votingChanges), changes: p.votingChanges },
    {
      id: "payout",
      title: t(`${P}.review.payout`),
      value: p.terms.payoutAddress ? short(p.terms.payoutAddress) : t(`${P}.review.newAddress`),
      status: status(p.payoutChanges),
      changes: p.payoutChanges !== false,
    },
  ];
}

export const signerLine = (p: UpdateRegistrarPreview) => t(p.ownerKey.kind === "keychain" ? `${P}.review.signerKeychain` : `${P}.review.signerAttached`);

/** A preview or send refusal as a sentence (§1.7's list, then §3.8's table). */
export function changeKeysErrorText(e: unknown, ops?: readonly ProTxOperation[]): string {
  if (isAppError(e) && e.code === "masternodes.protx_refused" && e.data?.kind === "reason") {
    if (e.data.reason === PAYEE_REUSE) return t(`${P}.payeeReuse`);
    if (e.data.reason === PENDING_DUP) return t(`${P}.keyInUse`);
    if (e.data.reason === DUP_KEY) return t(`${P}.keyListed`);
  }
  if (feeWentUp(e)) return t(`${P}.feeChanged`);
  return mnErrorText(e, ops);
}
