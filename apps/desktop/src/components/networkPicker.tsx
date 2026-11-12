// The network choice (network/choice.ts): a segmented Mainnet / Testnet control, shown at
// onboarding's first run and in Settings ▸ General. `useNetworkPicker` holds the choice and the
// switch (network/switch.ts): a network with a wallet asks first, once it knows whether a wallet
// has a transfer or a lock to finish; the switch then runs behind the window's switching card
// (`SwitchingCard`, above every page) and the page boots again on the new network. The control,
// the notes under it and the question are `NetworkPicker`.

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useId, useState } from "react";
import type { ChosenNetwork } from "../api/generated/ChosenNetwork";
import { keys, useBackend, useLockStatus, useNetworkChoice, useWallets } from "../api/queries";
import { ui, useUi } from "../app/ui";
import { NETWORK_CHOICES, NetworkText } from "../network/choice";
import { bootAgain, hasPendingLocks, switchOutcome } from "../network/switch";
import { DashButton } from "./basics";
import { Sheet } from "./sheet";

const chosenOf = (kind: string): ChosenNetwork | null => (kind === "mainnet" || kind === "testnet" ? kind : null);

export function useNetworkPicker() {
  const backend = useBackend();
  const client = useQueryClient();
  const query = useNetworkChoice();
  const wallets = useWallets();
  const lock = useLockStatus();
  const [confirming, setConfirming] = useState<ChosenNetwork | null>(null);
  // Each question reads afresh: one asked again does not go by the answer to the last.
  const [attempt, setAttempt] = useState(0);
  const [problem, setProblem] = useState<string | null>(null);
  // The network the window is moving to, until the page boots again (or a refusal ends it).
  const switching = useUi((s) => s.switching);
  const view = query.data ?? null;
  const pinned = view?.forcedByFlag ?? false;
  // A pinned window shows what the next start opens; any other, the network it is on.
  const current = view ? (pinned ? view.chosen : chosenOf(view.active.kind)) : null;
  const walletIds = wallets.data?.map((w) => w.walletId) ?? [];
  // Asks unless the network is known to have no wallet (the list may still be loading, or failed).
  const noWallet = wallets.isSuccess && walletIds.length === 0;
  // While the question is up: whether a wallet has a transfer or a lock to finish, read for this
  // question, and again when the wallet list arrives or changes. A list or a read that failed
  // counts as one.
  const pendingLocks = useQuery({
    queryKey: ["networkSwitchPending", attempt, walletIds],
    queryFn: () => hasPendingLocks(backend, walletIds),
    enabled: confirming !== null && wallets.isSuccess,
    gcTime: 0,
    staleTime: 0,
  });
  const checking = confirming !== null && (wallets.isPending || (wallets.isSuccess && pendingLocks.isPending));
  const pending = !checking && (wallets.isError || pendingLocks.isError || pendingLocks.data === true);
  const run = useMutation({
    mutationFn: (to: ChosenNetwork) => backend.switchNetwork(to),
    onMutate: (to) => {
      setProblem(null);
      ui.set({ switching: to });
    },
    onSuccess: (result, to) => {
      if (result.opened !== null) return void bootAgain(result.choice.chosen === to ? null : NetworkText.notSaved);
      // Pinned: saved for the next start, and the window stays.
      ui.set({ switching: null });
      client.setQueryData(keys.networkChoice, result.choice);
    },
    onError: (e, to) => {
      const outcome = switchOutcome(e, to, current ?? to, lock.data?.encrypted === true);
      if ("reboot" in outcome) return void bootAgain(outcome.reboot);
      ui.set({ switching: null });
      setProblem(outcome.problem);
    },
  });
  const choose = (to: ChosenNetwork) => {
    if (to === current || switching) return;
    if (pinned || noWallet) return run.mutate(to);
    setAttempt((n) => n + 1);
    setConfirming(to);
  };
  return {
    /** The network the control shows pressed; `null` while it is read, or when dw-app has no such command. */
    choice: current,
    available: view !== null,
    loading: query.isPending,
    /** The network the window is moving to, behind the switching card. */
    switching,
    /** The question before a switch: where to, whether it is still checking for unfinished
     * transfers (Switch waits), and whether there are some (or it could not tell). */
    confirming: confirming && { to: confirming, checking, pending },
    /** The window was started with `--network` (or a flag of one network): it only saves the choice. */
    pinned: view?.forcedByFlag ? view.active.kind : null,
    problem,
    choose,
    confirm: () => {
      if (!confirming || checking) return;
      setConfirming(null);
      run.mutate(confirming);
    },
    cancel: () => setConfirming(null),
  };
}

/** The card a network switch runs behind, above every page and onboarding: it stays until the
 * page boots again, or the switch is refused. */
export function SwitchingCard() {
  const switching = useUi((s) => s.switching);
  if (!switching) return null;
  return (
    <Sheet title={NetworkText.opening(switching)} width="small" testId="network-switching" closable={false} onClose={() => undefined}>
      <p className="sheet-text" role="status" aria-busy="true">
        {NetworkText.hint}
      </p>
    </Sheet>
  );
}

export type NetworkPickerState = ReturnType<typeof useNetworkPicker>;

export function NetworkPicker({ state, testId }: { state: NetworkPickerState; testId: string }) {
  const id = useId();
  const { choice, available, loading, switching, confirming } = state;
  return (
    <div className="network-picker" data-testid={testId}>
      <div className="settings-row">
        <span className="settings-row-title" id={`${id}-label`}>
          {NetworkText.title}
        </span>
        <div className="segmented" role="group" aria-labelledby={`${id}-label`}>
          {NETWORK_CHOICES.map((c) => (
            <button
              key={c.choice}
              type="button"
              className="segment"
              data-testid={`${testId}-${c.choice}`}
              aria-pressed={choice === c.choice}
              disabled={!available || switching !== null}
              onClick={() => state.choose(c.choice)}
            >
              {c.label}
            </button>
          ))}
        </div>
      </div>
      <p className="field-hint settings-hint">{NetworkText.hint}</p>
      {!loading && !available && (
        <p className="field-hint settings-hint" data-testid={`${testId}-unavailable`}>
          {NetworkText.unavailable}
        </p>
      )}
      {state.pinned && (
        <p className="field-hint settings-hint" data-testid={`${testId}-pinned`}>
          {NetworkText.pinned(state.pinned)}
        </p>
      )}
      {state.problem && (
        <p className="form-problem" role="alert" data-testid={`${testId}-problem`}>
          {state.problem}
        </p>
      )}
      {confirming && choice && (
        <Sheet title={NetworkText.confirmTitle(confirming.to)} width="small" testId="network-switch-sheet" onClose={state.cancel}>
          <p className="sheet-text">{NetworkText.confirmText(confirming.to, choice)}</p>
          {confirming.checking && (
            <p className="sheet-text" role="status" data-testid="network-switch-checking">
              {NetworkText.checking}
            </p>
          )}
          {confirming.pending && (
            <p className="sheet-text" data-testid="network-switch-pending">
              {NetworkText.confirmPending(choice)}
            </p>
          )}
          <div className="sheet-actions">
            <DashButton variant="tintedGray" data-autofocus onClick={state.cancel}>
              Cancel
            </DashButton>
            <DashButton variant="filledBlue" data-testid="network-switch-confirm" disabled={confirming.checking} onClick={state.confirm}>
              {NetworkText.confirm}
            </DashButton>
          </div>
        </Sheet>
      )}
    </div>
  );
}
