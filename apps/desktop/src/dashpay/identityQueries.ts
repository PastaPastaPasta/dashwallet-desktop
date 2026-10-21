// The Identities page's reads (DP6-01). Keys sit under `["dashpay", …]`, so the
// `Platform{Identities}` signal each write sends re-reads them (events.ts). The list shares the
// shell's `identities` key, so the chip, My Profile and this page show one answer.

import { useQuery } from "@tanstack/react-query";
import { keys, useBackend } from "../api/queries";

export const identityKeys = {
  detail: (walletId: string, identity: string) => ["dashpay", "identityDetail", walletId, identity] as const,
};

/** Every identity of the wallet, also without a main one (L3), unlike the shell's read. */
export function useIdentityList(walletId: string) {
  const b = useBackend();
  return useQuery({ queryKey: keys.identities(walletId), queryFn: () => b.identities(walletId) });
}

/** One identity's revision and keys from the wallet's store (D10); `null`: none open. */
export function useIdentityDetail(walletId: string, identity: string | null) {
  const b = useBackend();
  return useQuery({
    queryKey: identityKeys.detail(walletId, identity ?? ""),
    queryFn: () => b.identityDetail(walletId, identity!),
    enabled: identity !== null,
    retry: false,
  });
}
