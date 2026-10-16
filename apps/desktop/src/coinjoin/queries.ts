// The CoinJoin surfaces' reads and writes (U-06). Keys sit under `["coinjoin", …]` (api/queries.ts):
// the engine's `coinJoin` signal and `balances` re-read a wallet's status (events.ts), a settings
// write re-reads everything under the prefix.

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useState, useSyncExternalStore } from "react";
import type { CoinJoinSettings } from "../api/generated/CoinJoinSettings";
import type { CoinJoinStatus } from "../api/generated/CoinJoinStatus";
import type { NetworkInfo } from "../api/generated/NetworkInfo";
import { keys, useBackend } from "../api/queries";
import { useUi } from "../app/ui";
import { useDisplay } from "../format/display";
import { actionFailureText, coinjoinView, type CoinJoinView } from "./model";

/** dw-coinjoin's fixed values: they never change while the app runs. */
export function useCoinJoinLimits() {
  const b = useBackend();
  return useQuery({ queryKey: [...keys.coinjoinAll, "limits"], queryFn: () => b.coinjoinLimits(), staleTime: Infinity });
}

export function useCoinJoinSettings() {
  const b = useBackend();
  return useQuery({ queryKey: keys.coinjoinSettings, queryFn: () => b.coinjoinSettings() });
}

/** Writes the network's options, then re-reads every CoinJoin query: `enabled` and `rounds` move
 * each wallet's status. Rejects with the engine's error. */
export function useSaveCoinJoinSettings() {
  const b = useBackend();
  const client = useQueryClient();
  return async (settings: CoinJoinSettings) => {
    try {
      await b.setCoinjoinSettings(settings);
    } finally {
      await client.invalidateQueries({ queryKey: keys.coinjoinAll });
    }
  };
}

/** An amount as the CoinJoin surfaces show it ("1.2345 tDASH"). */
export function useCoinJoinAmount(network: NetworkInfo) {
  const display = useDisplay(network);
  return (duffs: number) => `${display.compact(duffs)} ${display.name}`;
}

/** The wallet's mixing status and its view (S1–S14); `retry` reads it again. */
export function useCoinJoinView(walletId: string, network: NetworkInfo): { view: CoinJoinView; status: CoinJoinStatus | undefined; retry: () => void } {
  const b = useBackend();
  const status = useQuery({ queryKey: keys.coinjoinStatus(walletId), queryFn: () => b.coinjoinStatus(walletId) });
  const discreet = useUi((s) => s.discreet);
  const display = useDisplay(network);
  const amount = useCoinJoinAmount(network);
  const view = coinjoinView({
    status: status.data,
    readFailed: status.isError,
    readError: status.error,
    amount,
    masked: discreet ? `${display.masked()} ${display.name}` : null,
  });
  return { view, status: status.data, retry: () => void status.refetch() };
}

/** Start or Stop Mixing for the wallet (D9): one call at a time; the failure's text until the next
 * press; the status is read again either way. */
export function useMixingAction(walletId: string, network: NetworkInfo) {
  const b = useBackend();
  const client = useQueryClient();
  const amount = useCoinJoinAmount(network);
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const run = async (kind: "start" | "stop") => {
    if (busy) return;
    setBusy(true);
    setProblem(null);
    try {
      await (kind === "start" ? b.startMixing(walletId) : b.stopMixing(walletId));
    } catch (e) {
      setProblem(actionFailureText(e, amount));
    } finally {
      await client.invalidateQueries({ queryKey: keys.coinjoinStatus(walletId) });
      setBusy(false);
    }
  };
  return { busy, problem, run };
}

// D1: the wallets whose stopped card the user dismissed. It stays hidden until mixing runs again,
// or the window reloads.
const dismissed = new Set<string>();
const listeners = new Set<() => void>();
let version = 0;

function setDismissed(walletId: string, on: boolean) {
  if (dismissed.has(walletId) === on) return;
  if (on) dismissed.add(walletId);
  else dismissed.delete(walletId);
  version++;
  for (const l of listeners) l();
}

/** Whether the Home card's stop notice was dismissed, and the dismiss. Mixing again clears it. */
export function useDismissedStop(walletId: string, state: CoinJoinStatus["state"] | undefined) {
  useSyncExternalStore(
    (l) => {
      listeners.add(l);
      return () => void listeners.delete(l);
    },
    () => version,
  );
  useEffect(() => {
    if (state !== undefined && state !== "idle") setDismissed(walletId, false);
  }, [walletId, state]);
  return { isDismissed: dismissed.has(walletId), dismiss: () => setDismissed(walletId, true) };
}
