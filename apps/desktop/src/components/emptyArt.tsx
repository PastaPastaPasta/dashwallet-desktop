// Small line drawings for empty screens. Stroke is currentColor so light and dark
// both read it; the colour is a design token on `.empty-state-art`.

import type { ReactNode } from "react";

export const EMPTY_ARTS = ["activity", "contacts", "requests", "address", "payment", "coinjoin", "notifications", "coins", "search", "peers", "wallet", "username"] as const;

export type EmptyArtId = (typeof EMPTY_ARTS)[number];

export function EmptyArt({ id, size = 64 }: { id: EmptyArtId; size?: number }) {
  return (
    <svg className="empty-state-art" viewBox="0 0 64 64" width={size} height={size} fill="none" stroke="currentColor" strokeWidth="1.75" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true" data-art={id}>
      {DRAW[id]}
    </svg>
  );
}

const DRAW: Record<EmptyArtId, ReactNode> = {
  activity: (
    <>
      <path d="M14 20h20" />
      <path d="M40 15l8 5-8 5" />
      <path d="M30 32h20" />
      <path d="M24 27l-8 5 8 5" />
      <path d="M14 44h18" />
      <path d="M40 39l8 5-8 5" />
    </>
  ),
  contacts: (
    <>
      <circle cx="32" cy="22" r="8" />
      <path d="M16 50c1.5-9 8-13 16-13s14.5 4 16 13" />
    </>
  ),
  requests: (
    <>
      <rect x="12" y="18" width="40" height="28" rx="3" />
      <path d="M14 20.5 32 34 50 20.5" />
    </>
  ),
  address: (
    <>
      <path d="M24 14h22a3 3 0 0 1 3 3v30a3 3 0 0 1-3 3H24" />
      <path d="M24 14c-5 1.5-8 3-8 7v22c0 4 3 5.5 8 7" />
      <path d="M24 14v36" />
      <path d="M30 26h12M30 34h12M30 42h8" />
    </>
  ),
  payment: (
    <>
      <rect x="14" y="12" width="26" height="40" rx="3" />
      <path d="M20 22h14M20 30h10" />
      <circle cx="44" cy="44" r="8" />
      <path d="M44 40.5v7M41.5 44h5" />
    </>
  ),
  coinjoin: (
    <>
      <circle cx="24" cy="32" r="12" />
      <circle cx="40" cy="32" r="12" />
    </>
  ),
  notifications: (
    <>
      <path d="M32 12a4 4 0 0 1 4 4c6 2 8 8 8 14v6l4 6H16l4-6v-6c0-6 2-12 8-14a4 4 0 0 1 4-4Z" />
      <path d="M28 50a4 4 0 0 0 8 0" />
    </>
  ),
  coins: (
    <>
      <ellipse cx="32" cy="42" rx="16" ry="6" />
      <path d="M16 34c0 3.3 7.2 6 16 6s16-2.7 16-6" />
      <ellipse cx="32" cy="26" rx="16" ry="6" />
    </>
  ),
  search: (
    <>
      <circle cx="28" cy="28" r="12" />
      <path d="M37 37 50 50" />
    </>
  ),
  peers: (
    <>
      <circle cx="16" cy="22" r="8" />
      <circle cx="48" cy="22" r="8" />
      <circle cx="32" cy="48" r="8" />
      <path d="M23 26 28 41M41 26 36 41M24 22h16" />
    </>
  ),
  wallet: (
    <>
      <rect x="10" y="18" width="44" height="30" rx="4" />
      <path d="M10 28h44" />
      <circle cx="46" cy="37" r="2.5" />
    </>
  ),
  username: (
    <>
      <circle cx="32" cy="32" r="16" />
      <circle cx="32" cy="32" r="6" />
      <path d="M38 26v12" />
    </>
  ),
};
