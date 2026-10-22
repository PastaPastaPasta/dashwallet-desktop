// The links a window kept for later (DP5-01 D11): a link opened before the network had a
// wallet, and every invitation, wait in the engine's link stash. After onboarding, an unlock
// and at launch the window offers them once ("Links you opened"). Pure: the sheet is
// screens/SavedLinks.tsx, the offer App.tsx.

import type { StashedLinkInfo } from "../api/generated/StashedLinkInfo";
import type { StashedLinkKindName } from "../api/generated/StashedLinkKindName";

export const SavedLinksText = {
  title: "Links you opened",
  intro: "You opened these links before your wallet was ready. Open one now, or remove it.",
  open: "Open",
  remove: "Remove",
  later: "Later",
  /** The toast for a link kept while there is no wallet yet. */
  saved: "Link saved. It opens once your wallet is set up.",
  /** The toast for an invitation opened during onboarding. */
  invitationSaved: "Invitation saved. It is waiting for you once your wallet is set up.",
  gone: "This link is no longer saved.",
  loadFailed: "Could not read the saved links.",
} as const;

export const KIND_TITLE: Record<StashedLinkKindName, string> = {
  payment: "Payment request",
  contact: "Contact",
  invitation: "DashPay invitation",
};

/** "Opened 5 minutes ago" for a link stashed at `stashedAt` (UNIX seconds), at `nowSecs`. */
export function openedAgo(stashedAt: number, nowSecs: number): string {
  const secs = Math.max(0, nowSecs - stashedAt);
  const unit = (n: number, one: string) => `${n} ${one}${n === 1 ? "" : "s"}`;
  if (secs < 60) return "Opened just now";
  if (secs < 3600) return `Opened ${unit(Math.floor(secs / 60), "minute")} ago`;
  if (secs < 86_400) return `Opened ${unit(Math.floor(secs / 3600), "hour")} ago`;
  const days = Math.floor(secs / 86_400);
  return days === 1 ? "Opened yesterday" : `Opened ${days} days ago`;
}

/** One row of the sheet. */
export interface SavedLinkRow {
  id: string;
  title: string;
  when: string;
  invitation: boolean;
}

/** The rows, newest first (the engine lists them oldest first). */
export function savedLinkRows(links: StashedLinkInfo[], nowSecs: number): SavedLinkRow[] {
  return [...links]
    .sort((a, b) => b.stashedAt - a.stashedAt || a.id.localeCompare(b.id))
    .map((l) => ({ id: l.id, title: KIND_TITLE[l.kind], when: openedAgo(l.stashedAt, nowSecs), invitation: l.kind === "invitation" }));
}

/** When the window offers the saved links: a trigger (launch, onboarding done, an unlock) is
 * pending, and the window shows a wallet with nothing in front of it. */
export function offerReady(s: { pending: boolean; hasWallet: boolean; locked: boolean; covered: boolean }): boolean {
  return s.pending && s.hasWallet && !s.locked && !s.covered;
}

/** What an offer opens, `links` oldest first as `stashed_links` lists them: after onboarding the
 * newest invitation's claim screen (DP5-03 D2), else the "Links you opened" dialog; nothing
 * without links. */
export function offerTarget(links: StashedLinkInfo[], afterOnboarding: boolean): { claim: string } | "sheet" | null {
  if (links.length === 0) return null;
  const invitation = afterOnboarding ? links.findLast((l) => l.kind === "invitation") : undefined;
  return invitation ? { claim: invitation.id } : "sheet";
}
