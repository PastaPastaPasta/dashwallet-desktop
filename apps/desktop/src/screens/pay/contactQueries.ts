// Pay ▸ To a contact's server state (DP3-06): the frequent strip and the picker, under
// `keys.contactsAll(wallet)` as Contacts' reads are, so the `platform` signals, a payment's
// `historyChanged` and every contact action's re-read refresh them (D3, D4). The payee's detail is
// Contacts' `useContactDetail`.

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { keys, useBackend } from "../../api/queries";
import { refreshAfter } from "../../contacts/queries";

/** The frequent strip's length (D3; the engine clamps to 1–20). */
const FREQUENT_LIMIT = 8;

export function useFrequentContacts(walletId: string, identity: string | null) {
  const b = useBackend();
  return useQuery({
    queryKey: [...keys.contactsAll(walletId), "frequent", identity] as const,
    queryFn: () => b.frequentContacts(walletId, identity!, FREQUENT_LIMIT),
    enabled: identity !== null,
  });
}

/** My Contacts and Contact Requests, by name, the search done by the engine. The last answer
 * stays on screen while a new search loads. */
export function usePayPicker(walletId: string, identity: string | null, text: string) {
  const b = useBackend();
  return useQuery({
    queryKey: [...keys.contactsAll(walletId), "payPicker", identity, text] as const,
    queryFn: () => b.dashpayContacts(walletId, identity!, { sections: ["contacts", "requests"], sort: "displayName", text: text || null }),
    enabled: identity !== null,
    placeholderData: (prev) => prev,
  });
}

/** Check again (D7): the engine looks for the locked payment's outcome; the contact is read again
 * whatever it found. */
export function useResolveLock(walletId: string, identity: string | null, contact: string | null) {
  const b = useBackend();
  const client = useQueryClient();
  return useMutation({
    mutationKey: ["resolveContactPaymentLock", walletId, identity, contact],
    mutationFn: (target: { identity: string; contact: string }) => b.resolveContactPaymentLock(walletId, target.identity, target.contact),
    onSettled: (_data, _error, target) => refreshAfter(client, walletId, target.identity, ["contacts"]),
  });
}
