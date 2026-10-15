// The Contacts screen's server state (DP2-07). The DashPay reads live under `["dashpay", …]`, so
// the event bridge's `platform` and `dashPayNews` invalidations re-read them, and the contacts
// list under `keys.contactsAll(wallet)`, which `historyChanged` re-reads too (D15). The network
// search is keyed by its prefix (latest wins) outside that prefix: a contact event need not
// repeat a DAPI query.

import { keepPreviousData, useQuery } from "@tanstack/react-query";
import type { ContactSortName } from "../api/generated/ContactSortName";
import { keys, useBackend } from "../api/queries";
import { contactsOwner, NETWORK_LIMIT } from "./model";

/** Whose contacts show (D2): the DashPay status (the shell's query) and, while DashPay's start is
 * incomplete, the identity list. */
export function useContactsOwner(walletId: string) {
  const b = useBackend();
  const status = useQuery({ queryKey: keys.dashpayStatus(walletId), queryFn: () => b.dashpayStatus(walletId) });
  const identities = useQuery({
    queryKey: keys.identities(walletId),
    queryFn: () => b.identities(walletId),
    enabled: status.data?.kind === "startupIncomplete",
  });
  return { owner: contactsOwner(status.data, identities.data), status };
}

/** Every section of `identity`'s contacts, sorted and filtered by the engine. The last answer
 * stays on screen while a new sort or search loads. */
export function useContactList(walletId: string, identity: string, sort: ContactSortName, text: string) {
  const b = useBackend();
  return useQuery({
    queryKey: [...keys.contactsAll(walletId), identity, sort, text] as const,
    queryFn: () => b.dashpayContacts(walletId, identity, { sections: [], sort, text: text || null }),
    placeholderData: keepPreviousData,
  });
}

/** Contacts waiting for an unlock or the DIP-15 rescan (D8). */
export function usePendingSetup(walletId: string) {
  const b = useBackend();
  return useQuery({ queryKey: ["dashpay", "pendingSetup", walletId] as const, queryFn: () => b.dashpayPendingSetupCount(walletId) });
}

/** DPNS usernames starting with `prefix` (D6); idle while `prefix` is `null`. A failure is shown
 * with Try again, not retried behind the user's back. */
export function useNetworkSearch(walletId: string, prefix: string | null) {
  const b = useBackend();
  return useQuery({
    queryKey: ["userSearch", walletId, prefix] as const,
    queryFn: () => b.searchUsers(walletId, prefix!, NETWORK_LIMIT),
    enabled: prefix !== null,
    retry: false,
    staleTime: 60_000,
  });
}
