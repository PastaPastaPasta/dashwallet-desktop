// The Contacts screen's server state (DP2-07). The DashPay reads live under `["dashpay", …]`, so
// the event bridge's `platform` and `dashPayNews` invalidations re-read them, and the contacts
// list under `keys.contactsAll(wallet)`, which `historyChanged` re-reads too (D15). The network
// search is keyed by its prefix (latest wins) outside that prefix: a contact event need not
// repeat a DAPI query.

import { useEffect, useMemo } from "react";
import { useInfiniteQuery, useQuery, useQueryClient } from "@tanstack/react-query";
import type { ActivityFilterName } from "../api/generated/ActivityFilterName";
import type { ContactSortName } from "../api/generated/ContactSortName";
import { keys, useBackend } from "../api/queries";
import { GrantModel } from "../tools/grant";
import { ContactActions, type Refresh } from "./actions";
import { contactsOwner, NETWORK_LIMIT, type ContactsOwner } from "./model";

/** Whose contacts show (D2): the DashPay status, the shell's query; `retry` reads it again (the
 * page shows "starting" while it runs). */
export function useContactsOwner(walletId: string): { owner: ContactsOwner; retry: () => void } {
  const b = useBackend();
  const status = useQuery({ queryKey: keys.dashpayStatus(walletId), queryFn: () => b.dashpayStatus(walletId) });
  const owner: ContactsOwner = status.isError && status.data === undefined ? { kind: "error" } : contactsOwner(status.data);
  return { owner, retry: () => void status.refetch() };
}

/** Every section of `identity`'s contacts, sorted and filtered by the engine. The last answer
 * stays on screen while a new sort or search loads; another identity's never does. */
export function useContactList(walletId: string, identity: string, sort: ContactSortName, text: string) {
  const b = useBackend();
  return useQuery({
    queryKey: [...keys.contactsAll(walletId), identity, sort, text] as const,
    queryFn: () => b.dashpayContacts(walletId, identity, { sections: [], sort, text: text || null }),
    placeholderData: (prev, prevQuery) => (prevQuery?.queryKey[3] === identity ? prev : undefined),
  });
}

/** Contacts waiting for an unlock or the DIP-15 rescan (D8). */
export function usePendingSetup(walletId: string) {
  const b = useBackend();
  return useQuery({ queryKey: ["dashpay", "pendingSetup", walletId] as const, queryFn: () => b.dashpayPendingSetupCount(walletId) });
}

/** DPNS usernames starting with `prefix` (D6); idle while `prefix` is `null`. A failure is shown
 * with Try again, not retried behind the user's back. The hits' relations are the main identity's,
 * so it is part of the key (D21). */
export function useNetworkSearch(walletId: string, identity: string, prefix: string | null) {
  const b = useBackend();
  return useQuery({
    queryKey: [...searchKey(walletId, identity), prefix] as const,
    queryFn: () => b.searchUsers(walletId, prefix!, NETWORK_LIMIT),
    enabled: prefix !== null,
    retry: false,
    staleTime: 60_000,
  });
}

/** The network search's queries of `identity` (D6: Unignore drops them, DP2-09 D6). */
const searchKey = (walletId: string, identity: string) => ["userSearch", walletId, identity] as const;

/** One contact's detail: private details, publish state, request times (DP2-09 D1). Under
 * `contactsAll`, so every re-read of the list re-reads it. */
export function useContactDetail(walletId: string, identity: string, contact: string | null) {
  const b = useBackend();
  return useQuery({
    queryKey: [...keys.contactsAll(walletId), "detail", identity, contact] as const,
    queryFn: () => b.dashpayContact(walletId, identity, contact!),
    enabled: contact !== null,
  });
}

/** A contact's payments, a page at a time (DP2-09 D8); under `contactsAll`, so `historyChanged`
 * and the contact and payment signals re-read it. */
export function useContactActivity(walletId: string, identity: string, contact: string, filter: ActivityFilterName, enabled: boolean) {
  const b = useBackend();
  return useInfiniteQuery({
    queryKey: [...keys.contactsAll(walletId), "activity", identity, contact, filter] as const,
    queryFn: ({ pageParam }) => b.contactActivity(walletId, identity, contact, pageParam, filter),
    initialPageParam: null as string | null,
    getNextPageParam: (last) => last.nextCursor,
    enabled,
  });
}

/** The wallet's identities (the shell's query): whether the main one has its DashPay keys. */
export function useIdentities(walletId: string) {
  const b = useBackend();
  return useQuery({ queryKey: keys.identities(walletId), queryFn: () => b.identities(walletId) });
}

/** The credit costs, for the Enable DashPay banner's fee (DP2-09 D10). */
export function useCostTable(walletId: string) {
  const b = useBackend();
  return useQuery({ queryKey: ["dashpay", "costTable", walletId] as const, queryFn: () => b.creditsCostTable(walletId), staleTime: 600_000 });
}

/** The actions one surface runs for `identity` (DP2-09 D16): one model and grant prompt per
 * mount, cancelled when the surface goes or names another identity. */
export function useContactActions(walletId: string, identity: string): ContactActions {
  const b = useBackend();
  const client = useQueryClient();
  const actions = useMemo(() => {
    const refresh = (what: Refresh[]) => {
      for (const w of what)
        void client.invalidateQueries({
          queryKey: w === "contacts" ? keys.dashpayAll : w === "identities" ? keys.identities(walletId) : searchKey(walletId, identity),
        });
    };
    return new ContactActions(b, new GrantModel(b), walletId, identity, refresh);
  }, [b, client, walletId, identity]);
  useEffect(() => () => actions.cancel(), [actions]);
  return actions;
}
