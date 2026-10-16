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
import { actionFailureText, coinjoinView, sameMarks, START_WINDOW_MS, type CoinJoinView, type MixerMarks, type SeenStart } from "./model";

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
  const start = useSyncExternalStore(subscribe, () => starts.get(walletId));
  const [, tick] = useState(0);
  const view = coinjoinView({ status: status.data, readFailed: status.isError, readError: status.error, amount, masked, total, start, now: Date.now() });
  // Mixing again ends the dismissal of the last stop (D1), whichever surface sees it run. A stopped
  // mixer's marks are what a start starts from, once the balance has been read too (D16).
  const state = status.data?.state;
  const balances = status.data?.balances;
  useEffect(() => {
    if (state === "idle" && balances) setStopped(walletId, { balances, total });
    else if (state !== undefined) {
      setDismissed(walletId, false);
      setStarted(walletId);
    }
  }, [walletId, state, balances, total]);
  // Starting… ends on its own when its window does (D16).
  const until = view.state === "starting" && start?.since !== undefined ? start.since + START_WINDOW_MS : null;
  useEffect(() => {
    if (until === null) return;
    const t = window.setTimeout(() => tick((n) => n + 1), Math.max(0, until - Date.now()) + 50);
    return () => window.clearTimeout(t);
  }, [until]);
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
// D16: each wallet's start as the CoinJoin surfaces saw it, from the last read of its mixer stopped.
let starts: ReadonlyMap<string, SeenStart> = new Map();
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

/** The mixer was read stopped, with the wallet's total once it has been read too. */
function setStopped(walletId: string, marks: MixerMarks) {
  const was = starts.get(walletId);
  if (was && was.since === undefined && sameMarks(was.marks, marks)) return;
  starts = new Map(starts).set(walletId, { marks });
  notify();
}

/** The mixer was read running: the first such read after a stopped one starts the window. */
function setStarted(walletId: string) {
  const was = starts.get(walletId);
  if (!was || was.since !== undefined) return;
  starts = new Map(starts).set(walletId, { ...was, since: Date.now() });
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
