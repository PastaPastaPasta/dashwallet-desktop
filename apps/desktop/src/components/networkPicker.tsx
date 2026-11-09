// The network choice (network/choice.ts): a segmented Mainnet / Testnet control, shown at
// onboarding's first run and in Settings ▸ General. `useNetworkPicker` holds the choice and the
// switch (network/switch.ts): a network with a wallet asks first; the switch then runs behind a
// blocking card and the page boots again on the new network. The control, the notes under it and
// those two sheets are `NetworkPicker`.

import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useId, useState } from "react";
import type { ChosenNetwork } from "../api/generated/ChosenNetwork";
import { keys, useBackend, useNetworkChoice, useWallets } from "../api/queries";
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
  const [confirming, setConfirming] = useState<{ to: ChosenNetwork; pending: boolean } | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  // The network the window is moving to, until the page boots again (or a refusal ends it).
  const [switching, setSwitching] = useState<ChosenNetwork | null>(null);
  const view = query.data ?? null;
  const pinned = view?.forcedByFlag ?? false;
  // A pinned window shows what the next start opens; any other, the network it is on.
  const current = view ? (pinned ? view.chosen : chosenOf(view.active.kind)) : null;
  const walletIds = wallets.data?.map((w) => w.walletId) ?? [];
  const run = useMutation({
    mutationFn: (to: ChosenNetwork) => backend.switchNetwork(to),
    onMutate: (to) => {
      setProblem(null);
      setSwitching(to);
    },
    onSuccess: (result) => {
      if (result.opened !== null) return void bootAgain();
      // Pinned: saved for the next start, and the window stays.
      setSwitching(null);
      client.setQueryData(keys.networkChoice, result.choice);
    },
    onError: (e, to) => {
      const outcome = switchOutcome(e, to, current ?? to);
      if ("reboot" in outcome) return void bootAgain(outcome.reboot);
      setSwitching(null);
      setProblem(outcome.problem);
    },
  });
  const choose = (to: ChosenNetwork) => {
    if (to === current || switching) return;
    if (pinned || walletIds.length === 0) return run.mutate(to);
    setConfirming({ to, pending: false });
    void hasPendingLocks(backend, walletIds).then((pending) => setConfirming((c) => (c?.to === to ? { to, pending } : c)));
  };
  return {
    /** The network the control shows pressed; `null` while it is read, or when dw-app has no such command. */
    choice: current,
    available: view !== null,
    loading: query.isPending,
    /** The network the window is moving to, behind the blocking card. */
    switching,
    confirming,
    /** The window was started with `--network` (or a flag of one network): it only saves the choice. */
    pinned: pinned && view ? view.active.kind : null,
    problem,
    choose,
    confirm: () => {
      if (!confirming) return;
      setConfirming(null);
      run.mutate(confirming.to);
    },
    cancel: () => setConfirming(null),
  };
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
          {confirming.pending && (
            <p className="sheet-text" data-testid="network-switch-pending">
              {NetworkText.confirmPending(choice)}
            </p>
          )}
          <div className="sheet-actions">
            <DashButton variant="tintedGray" data-autofocus onClick={state.cancel}>
              Cancel
            </DashButton>
            <DashButton variant="filledBlue" data-testid="network-switch-confirm" onClick={state.confirm}>
              {NetworkText.confirm}
            </DashButton>
          </div>
        </Sheet>
      )}
      {switching && (
        <Sheet title={NetworkText.opening(switching)} width="small" testId="network-switching" closable={false} onClose={() => undefined}>
          <p className="sheet-text" role="status" aria-busy="true">
            {NetworkText.hint}
          </p>
        </Sheet>
      )}
    </div>
  );
}
