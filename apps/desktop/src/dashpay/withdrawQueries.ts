// DP6-02's reads. Keys sit under `["dashpay", …]`, so every `Platform` signal and every settled
// withdrawal re-reads them (events.ts); the quote sits under DP1-09's `quote` key, so a balance
// change re-reads it too (as DP1-10 D18).

import { useQuery } from "@tanstack/react-query";
import type { WithdrawAmount } from "../api/generated/WithdrawAmount";
import { useBackend } from "../api/queries";
import { dashpayKeys } from "./queries";
import { amountKey, pollWhilePending } from "./withdraw";

export const withdrawKeys = {
  quote: (walletId: string, identity: string, amount: WithdrawAmount) => [...dashpayKeys.quote(walletId), "withdraw", identity, amountKey(amount)] as const,
  recent: (walletId: string, identity: string) => ["dashpay", "withdrawals", walletId, identity] as const,
};

export function useWithdrawQuote(walletId: string, identity: string | null, amount: WithdrawAmount | null) {
  const b = useBackend();
  return useQuery({
    queryKey: withdrawKeys.quote(walletId, identity ?? "", amount ?? { kind: "all" }),
    queryFn: () => b.creditsWithdrawQuote(walletId, identity!, amount!),
    enabled: identity !== null && amount !== null,
    retry: false,
  });
}

/** D11: the identity's withdrawals, read on signals and every minute while one is pending or
 * `unconfirmed` (D23) is not listed. `enabled` false: only what another reader has cached. */
export function useWithdrawals(walletId: string, identity: string, unconfirmed: string | null = null, enabled = true) {
  const b = useBackend();
  return useQuery({
    queryKey: withdrawKeys.recent(walletId, identity),
    queryFn: () => b.creditsWithdrawals(walletId, identity),
    enabled,
    retry: false,
    refetchInterval: (q) => pollWhilePending(q.state.data, unconfirmed),
  });
}

