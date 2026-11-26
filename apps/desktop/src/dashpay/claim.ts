// The invitation claim screen's view model (DP5-03): the states before the claim starts (C3–C9),
// the inviter's card, the request's recipient (D6), the claim's refusals and its Done note. The
// claim itself is the registration wizard's invitation mode (wizard.ts), and its progress and
// result are DP1-09's screens for the claim's row (registration.ts), so only what differs is here.

import { hasCode } from "../api/backend";
import type { InvitationInviter } from "../api/generated/InvitationInviter";
import type { InvitationStatus } from "../api/generated/InvitationStatus";
import type { RegistrationStatus } from "../api/generated/RegistrationStatus";
import { startErrorText } from "./registration";
import { messageTable, t } from "../i18n";
import { CLAIM_REFUSAL, withTable } from "./status";
import { platformHoldText } from "../api/platformHold";

/** Where the claim screen was opened from (D2, D3): onboarding's replayed link, or a link opened
 * in a wallet. */
export type ClaimEntry = "welcome" | "join";

const claim = <K extends string>(prefix: string, keys: readonly K[]) => messageTable(prefix ? `dashpay.claim.${prefix}` : "dashpay.claim", keys);

export const CLAIM_COPY = withTable(claim("", ["checking", "checkFailed", "paid", "kept", "accept", "notNow", "dismiss", "retry"]), {
  title: claim("title", ["welcome", "join"]),
  get hasIdentity() {
    return CLAIM_REFUSAL.hasIdentity;
  },
  claimed: CLAIM_REFUSAL.claimed,
  expired: CLAIM_REFUSAL.expired,
  invalid: {
    get malformed() {
      return CLAIM_REFUSAL.invalid;
    },
    wrongNetwork: (network: string) => t("dashpay.claim.wrongNetwork", { network }),
    get assetLockNotFound() {
      return t("dashpay.claim.assetLockNotFound");
    },
  },
  invited: withTable(claim("invited", ["from", "unnamed"]), {
    // D4 (F7): DPNS proves the name, not who sent the link.
    checked: (username: string) => t("dashpay.claim.invited.checked", { username: `@${username}` }),
  }),
  confirm: withTable(claim("confirm", ["recipientId", "claim"]), {
    // D6 (Opus-h r1 n3).
    // `who`: "@username", or the identity id of an inviter without one.
    recipient: (who: string) => t("dashpay.claim.confirm.recipient", { who }),
    request: (who: string) => t("dashpay.claim.confirm.request", { who }),
  }),
  name: {
    // D15: invite-limited names.
    get contestedRefused() {
      return t("dashpay.claim.contestedRefused");
    },
  },
  paste: claim("paste", ["open", "title", "field", "refused", "cancel", "ok"]),
});

/** The claim screen's states before a claim (C1 and C2 are the claim row's own screens). */
export type ClaimState = "hasIdentity" | "checking" | "checkFailed" | "claimed" | "expired" | "invalid" | "invited";

export type ClaimActionKind = "accept" | "notNow" | "dismiss" | "retry";

export interface ClaimAction {
  kind: ClaimActionKind;
  label: string;
}

export interface InviterCard {
  name: string | null;
  username: string | null;
  identity: string;
  avatarUrl: string | null;
  line: string;
}

export interface ClaimModel {
  state: ClaimState;
  title: string;
  lines: string[];
  busy: boolean;
  inviter: InviterCard | null;
  /** The first is the primary action. */
  actions: ClaimAction[];
}

export interface ClaimInputs {
  entry: ClaimEntry;
  /** The wallet has a DashPay identity (C3). */
  hasIdentity: boolean;
  status: InvitationStatus | undefined;
  /** The `invitation_status` read failed (C5). */
  statusError: unknown;
  /** The window's network, as the user knows it (C8). */
  network: string;
}

const act = (kind: ClaimActionKind): ClaimAction => ({ kind, label: CLAIM_COPY[kind] });

/** D5 (F7): the wallet's claim of `linkId`, read before the status says `claimed`. A claim that
 * failed for good is not the claim screen's: the link's own state decides then. */
export function claimRow(rows: RegistrationStatus[] | undefined, linkId: string): RegistrationStatus | undefined {
  return rows?.findLast((r) => r.invitation?.linkId === linkId && !(r.phase === "failed" && r.failure?.retryable === false));
}

/** D24: a claim stopped because another identity of the wallet's seed holds its keys, which the
 * wallet has not found yet. The wallet has an identity then (C3): the engine refuses the next claim
 * of any link while the row is kept, so the row is not discarded for a new one. */
export function claimFoundIdentity(r: RegistrationStatus): boolean {
  return r.failure?.retryable === false && r.failure.code === "invitation.already_has_identity";
}

/** C3: the wallet's identity is `known`, or a claim was refused for one not found yet (D24). */
export function claimHasIdentity(known: boolean, rows: RegistrationStatus[] | undefined): boolean {
  return known || (rows?.some(claimFoundIdentity) ?? false);
}

/** D4: what the card says of the inviter; never "verified". */
export function inviterCard(inviter: InvitationInviter | null): InviterCard | null {
  if (!inviter) return null;
  return {
    name: inviter.displayName,
    username: inviter.username,
    identity: inviter.identity,
    avatarUrl: inviter.avatarUrl,
    line: inviter.username ? CLAIM_COPY.invited.checked(inviter.username) : CLAIM_COPY.invited.unnamed,
  };
}

/** The claim screen for the inputs (C3–C9, the first that holds). */
export function claimModel(i: ClaimInputs): ClaimModel {
  const title = CLAIM_COPY.title[i.entry];
  const base = (state: ClaimState, lines: string[], actions: ClaimAction[], rest: Partial<ClaimModel> = {}): ClaimModel => ({
    state,
    title,
    lines,
    busy: false,
    inviter: null,
    actions,
    ...rest,
  });
  if (i.hasIdentity) return base("hasIdentity", [CLAIM_COPY.hasIdentity], [act("dismiss"), act("notNow")]);
  if (i.statusError) return base("checkFailed", [CLAIM_COPY.checkFailed, platformHoldText(i.statusError, false) ?? startErrorText(i.statusError, String)], [act("retry"), act("notNow")]);
  const s = i.status;
  if (!s) return base("checking", [CLAIM_COPY.checking], [act("notNow")], { busy: true });
  switch (s.kind) {
    case "claimed":
      return base("claimed", [CLAIM_COPY.claimed.title, CLAIM_COPY.claimed.text], [act("dismiss")]);
    case "expired":
      return base("expired", [CLAIM_COPY.expired.title, CLAIM_COPY.expired.text], [act("notNow")]);
    case "invalid": {
      const lines = {
        malformed: CLAIM_COPY.invalid.malformed,
        wrongNetwork: CLAIM_COPY.invalid.wrongNetwork(i.network),
        assetLockNotFound: CLAIM_COPY.invalid.assetLockNotFound,
      };
      return base("invalid", [lines[s.reason]], s.reason === "assetLockNotFound" ? [act("retry"), act("dismiss")] : [act("dismiss")]);
    }
    case "valid":
      return base("invited", [CLAIM_COPY.paid], [act("accept"), act("notNow"), act("dismiss")], { inviter: inviterCard(s.inviter) });
  }
}

/** D6: who the request goes to, before Claim; `null` without an inviter. */
export function recipientLines(inviter: InvitationInviter | null): { text: string; identity: string } | null {
  if (!inviter) return null;
  return { text: CLAIM_COPY.confirm.recipient(handleOf(inviter)), identity: inviter.identity };
}

/** How the copy names the inviter: "@username", else the identity id (no "@": it is no name). */
export function handleOf(inviter: InvitationInviter): string {
  return inviter.username ? `@${inviter.username}` : inviter.identity;
}

/** D11: the discard of a claim row that may have reached the network was refused ("Choose
 * another username" on its retry screen); the row is continued instead. */
export const CLAIM_KEPT = Symbol("claimKept");

/** C12: why a claim's start or resume was refused. */
export function claimErrorText(e: unknown, amount: (duffs: number) => string): string {
  if (e === CLAIM_KEPT) return CLAIM_COPY.kept;
  if (hasCode(e, "invitation.claimed")) return t("dashpay.refusal.claimedSentence");
  if (hasCode(e, "invitation.expired")) return t("dashpay.refusal.expiredSentence");
  if (hasCode(e, "invitation.invalid")) return CLAIM_COPY.invalid.malformed;
  if (hasCode(e, "invitation.already_has_identity")) return CLAIM_COPY.hasIdentity;
  if (hasCode(e, "name.unavailable_for_invite")) return CLAIM_COPY.name.contestedRefused;
  if (hasCode(e, "registration.funding_insufficient")) return t("dashpay.claim.notCovered");
  return startErrorText(e, amount);
}
