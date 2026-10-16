// The CoinJoin surfaces' reads and writes (U-06). Keys sit under `["coinjoin", …]` (api/queries.ts):
// the engine's `coinJoin` signal and `balances` re-read a wallet's status (events.ts), a settings
// write re-reads everything under the prefix.

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useState, useSyncExternalStore } from "react";
import type { CoinJoinSettings } from "../api/generated/CoinJoinSettings";
import type { CoinJoinStatus } from "../api/generated/CoinJoinStatus";
import type { NetworkInfo } from "../api/generated/NetworkInfo";
import { keys, useBackend, useBalance } from "../api/queries";
import { useUi } from "../app/ui";
import { useDisplay } from "../format/display";
import { actionFailureText, coinjoinView, sameMarks, type CoinJoinView, type MixerMarks } from "./model";

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

/** What discreet mode shows in place of an amount; `null` while amounts show. */
export function useMaskedAmount(network: NetworkInfo): string | null {
  const discreet = useUi((s) => s.discreet);
  const display = useDisplay(network);
  return discreet ? `${display.masked()} ${display.name}` : null;
}

/** The wallet's mixing status and its view (S1–S14); `retry` reads it again. */
export function useCoinJoinView(walletId: string, network: NetworkInfo): { view: CoinJoinView; status: CoinJoinStatus | undefined; retry: () => void } {
  const b = useBackend();
  const status = useQuery({ queryKey: keys.coinjoinStatus(walletId), queryFn: () => b.coinjoinStatus(walletId) });
  const amount = useCoinJoinAmount(network);
  const masked = useMaskedAmount(network);
  const total = useBalance(walletId).data?.total;
  const atStart = useSyncExternalStore(subscribe, () => resting.get(walletId));
  const view = coinjoinView({ status: status.data, readFailed: status.isError, readError: status.error, amount, masked, total, atStart });
  // Mixing again ends the dismissal of the last stop (D1), whichever surface sees it run; a stopped
  // mixer's marks are what a start starts from (D16).
  const state = status.data?.state;
  const balances = status.data?.balances;
  useEffect(() => {
    if (state === "idle" && balances) setResting(walletId, { balances, total });
    else if (state !== undefined) setDismissed(walletId, false);
  }, [walletId, state, balances, total]);
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

// Window-wide state of the CoinJoin surfaces, so the Home card and the page agree. Replaced, never
// mutated, so each is its own store snapshot.
// D1: the wallets whose stopped card the user dismissed. It stays hidden until any CoinJoin surface
// sees mixing run again (`useCoinJoinView`), or the window reloads.
let dismissed: ReadonlySet<string> = new Set();
// D16: each wallet's marks when a CoinJoin surface last read its mixer stopped.
let resting: ReadonlyMap<string, MixerMarks> = new Map();
const listeners = new Set<() => void>();
const notify = () => {
  for (const l of listeners) l();
};

function setDismissed(walletId: string, on: boolean) {
  if (dismissed.has(walletId) === on) return;
  const next = new Set(dismissed);
  if (on) next.add(walletId);
  else next.delete(walletId);
  dismissed = next;
  notify();
}

function setResting(walletId: string, marks: MixerMarks) {
  const was = resting.get(walletId);
  if (was && sameMarks(was, marks)) return;
  resting = new Map(resting).set(walletId, marks);
  notify();
}

function subscribe(l: () => void) {
  listeners.add(l);
  return () => void listeners.delete(l);
}

/** Whether the Home card's stop notice was dismissed, and the dismiss. */
export function useDismissedStop(walletId: string) {
  const wallets = useSyncExternalStore(subscribe, () => dismissed);
  return { isDismissed: wallets.has(walletId), dismiss: () => setDismissed(walletId, true) };
}
