// The Contacts screen's server state (DP2-07). The DashPay reads live under `["dashpay", …]`, so
// the event bridge's `platform` and `dashPayNews` invalidations re-read them, and the contacts
// list under `keys.contactsAll(wallet)`, which `historyChanged` re-reads too (D15). The network
// search is keyed by its prefix (latest wins) outside that prefix: a contact event need not
// repeat a DAPI query.

import { useEffect, useMemo } from "react";
import { queryOptions, useInfiniteQuery, useQuery, useQueryClient, type QueryClient } from "@tanstack/react-query";
import type { ActivityFilterName } from "../api/generated/ActivityFilterName";
import type { ContactSortName } from "../api/generated/ContactSortName";
import type { Backend } from "../api/backend";
import { keys, useBackend } from "../api/queries";
import { GrantModel } from "../tools/grant";
import { ContactActions, type Refresh } from "./actions";
import { nextRead } from "./add";
import { contactsOwner, NETWORK_LIMIT, type ContactsOwner } from "./model";
import { enableBanner, type EnableBanner } from "./pane";

/** Whose contacts show (D2): the DashPay status, the shell's query; `retry` reads it again (the
 * page shows "starting" while it runs). */
export function useContactsOwner(walletId: string): { owner: ContactsOwner; retry: () => void } {
  const b = useBackend();
  const status = useQuery({ queryKey: keys.dashpayStatus(walletId), queryFn: () => b.dashpayStatus(walletId) });
  const owner: ContactsOwner = status.isError && status.data === undefined ? { kind: "error" } : contactsOwner(status.data);
  return { owner, retry: () => void status.refetch() };
}

/** Every section of `identity`'s contacts, sorted and filtered by the engine, with the `nextRead`
 * number the read took when it began (DP2-08 D38). */
export const contactListQuery = (b: Pick<Backend, "dashpayContacts">, walletId: string, identity: string, sort: ContactSortName, text: string) =>
  queryOptions({
    queryKey: [...keys.contactsAll(walletId), identity, sort, text] as const,
    queryFn: async () => {
      const seq = nextRead();
      return { ...(await b.dashpayContacts(walletId, identity, { sections: [], sort, text: text || null })), seq };
    },
  });

/** `contactListQuery`'s answer. The last answer stays on screen while a new sort or search loads;
 * another identity's never does. */
export function useContactList(walletId: string, identity: string, sort: ContactSortName, text: string) {
  const b = useBackend();
  return useQuery({
    ...contactListQuery(b, walletId, identity, sort, text),
    placeholderData: (prev, prevQuery) => (prevQuery?.queryKey[3] === identity ? prev : undefined),
  });
}

/** Contacts waiting for an unlock or the DIP-15 rescan (D8). */
export function usePendingSetup(walletId: string) {
  const b = useBackend();
  return useQuery({ queryKey: ["dashpay", "pendingSetup", walletId] as const, queryFn: () => b.dashpayPendingSetupCount(walletId) });
}

/** The network search's queries of `identity`: Unignore drops them (DP2-09 D6). */
const searchKey = (walletId: string, identity: string) => ["userSearch", walletId, identity] as const;

/** DPNS usernames starting with `prefix` (D6), each with the `nextRead` number taken when they
 * arrived (DP2-08 D38). The hits' relations are the main identity's, so it is part of the key (D21). */
export const networkSearchQuery = (b: Pick<Backend, "searchUsers">, walletId: string, identity: string, prefix: string | null) =>
  queryOptions({
    queryKey: [...searchKey(walletId, identity), prefix] as const,
    queryFn: async () => {
      const hits = await b.searchUsers(walletId, prefix!, NETWORK_LIMIT);
      const readSeq = nextRead();
      return hits.map((h) => ({ ...h, readSeq }));
    },
  });

/** `networkSearchQuery`; idle while `prefix` is `null`. A failure is shown with Try again, not
 * retried behind the user's back. */
export function useNetworkSearch(walletId: string, identity: string, prefix: string | null) {
  const b = useBackend();
  return useQuery({
    ...networkSearchQuery(b, walletId, identity, prefix),
    enabled: prefix !== null,
    retry: false,
    staleTime: 60_000,
  });
}

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
export function useContactActivity(walletId: string, identity: string, contact: string, filter: ActivityFilterName) {
  const b = useBackend();
  return useInfiniteQuery({
    queryKey: [...keys.contactsAll(walletId), "activity", identity, contact, filter] as const,
    queryFn: ({ pageParam }) => b.contactActivity(walletId, identity, contact, pageParam, filter),
    initialPageParam: null as string | null,
    getNextPageParam: (last) => last.nextCursor,
  });
}

/** Reads again what an action changed (DP2-09 D16). A contacts read still on its first answer is
 * reset, so one begun after the action follows: TanStack 5.104.1 cancels a running read on
 * invalidation only once its query holds data, else hands back that read, which may predate the
 * action (DP2-08 D38). */
export function refreshAfter(client: QueryClient, walletId: string, identity: string, what: Refresh[]) {
  for (const w of what) {
    if (w === "contacts") void client.resetQueries({ queryKey: keys.contactsAll(walletId), predicate: (q) => q.state.data === undefined && q.state.fetchStatus !== "idle" });
    void client.invalidateQueries({
      queryKey: w === "contacts" ? keys.dashpayAll : w === "identities" ? keys.identities(walletId) : searchKey(walletId, identity),
    });
  }
}

/** The actions one surface runs for `identity` (DP2-09 D16): one model and grant prompt per
 * mount, cancelled when the surface goes or names another identity. */
export function useContactActions(walletId: string, identity: string): ContactActions {
  const b = useBackend();
  const client = useQueryClient();
  const actions = useMemo(
    () => new ContactActions(b, new GrantModel(b), walletId, identity, (what) => refreshAfter(client, walletId, identity, what)),
    [b, client, walletId, identity],
  );
  useEffect(() => () => actions.cancel(), [actions]);
  return actions;
}

/** Whether `identity` has its DashPay keys, from the shell's identities query; `undefined`
 * while unknown (DP2-09 D2, D10). */
export function useHasKeys(walletId: string, identity: string): boolean | undefined {
  const b = useBackend();
  const identities = useQuery({ queryKey: keys.identities(walletId), queryFn: () => b.identities(walletId) });
  return identities.data?.find((i) => i.identity === identity)?.hasDashpayKeys;
}

/** The Enable DashPay banner (DP2-09 D10) for the main `identity`, with the fee read only while
 * the banner shows. */
export function useEnableBanner(walletId: string, identity: string): { banner: EnableBanner | null; hasKeys: boolean | undefined } {
  const b = useBackend();
  const hasKeys = useHasKeys(walletId, identity);
  const costs = useQuery({
    queryKey: keys.costTable(walletId),
    queryFn: () => b.creditsCostTable(walletId),
    staleTime: 600_000,
    enabled: hasKeys === false,
  });
  return { banner: enableBanner(identity, hasKeys, costs.data), hasKeys };
}

/** Whether `identity` may send a request to `contact` (DP2-08 D4); under `["dashpay"]`, so the
 * DashPay signals and every action's re-read check again (D14). Idle while `contact` is `null`. */
export function useEligibility(walletId: string, identity: string | null, contact: string | null) {
  const b = useBackend();
  return useQuery({
    queryKey: ["dashpay", "eligibility", walletId, identity, contact] as const,
    queryFn: () => b.dashpayEligibility(walletId, identity!, contact!),
    enabled: identity !== null && contact !== null,
    retry: false,
  });
}

/** The identity's `dashpay://user` link for My QR (DP2-08 D12); under `["dashpay"]`, so a new
 * main name re-reads it. */
export function useMyUserLink(walletId: string, identity: string) {
  const b = useBackend();
  return useQuery({
    queryKey: ["dashpay", "myUserLink", walletId, identity] as const,
    queryFn: () => b.dashpayMyUserLink(walletId, identity),
    retry: false,
  });
}

/** How this OS scans a screen region (DP2-08 D10); it never changes. */
export function useScreenScanMode() {
  const b = useBackend();
  return useQuery({ queryKey: ["screenScanMode"] as const, queryFn: () => b.screenScanMode(), staleTime: Infinity });
}
