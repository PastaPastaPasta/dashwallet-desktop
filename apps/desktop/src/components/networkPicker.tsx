// The network choice (network/choice.ts): a segmented Mainnet / Testnet control, shown at
// onboarding and in Settings ▸ General. `useNetworkPicker` holds the choice and its save; the
// control and the notes under it are `NetworkPicker`.

import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useId } from "react";
import type { NetworkChoice } from "../api/pending";
import { errorMessage } from "../api/backend";
import { keys, useBackend, useNetworkChoice } from "../api/queries";
import { NETWORK_CHOICES, NetworkText } from "../network/choice";

export function useNetworkPicker() {
  const backend = useBackend();
  const client = useQueryClient();
  const query = useNetworkChoice();
  const save = useMutation({
    mutationFn: (choice: NetworkChoice) => backend.setNetworkChoice(choice),
    onSuccess: (view) => client.setQueryData(keys.networkChoice, view),
  });
  const view = query.data ?? null;
  const choice = view?.chosen ?? null;
  return {
    /** The saved choice; `null` while it is read, or when dw-app has no such command. */
    choice,
    available: view !== null,
    loading: query.isPending,
    /** The saved choice is not the network this window runs on. */
    restart: view?.restartNeeded ?? false,
    /** The network this window was started on by `--network` (it wins over the choice). */
    pinned: view?.forcedByFlag ? view.launched.kind : null,
    saving: save.isPending,
    problem: save.isError ? `${NetworkText.notSaved} ${errorMessage(save.error)}` : null,
    choose: (c: NetworkChoice) => save.mutate(c),
  };
}

export type NetworkPickerState = ReturnType<typeof useNetworkPicker>;

export function NetworkPicker({ state, testId }: { state: NetworkPickerState; testId: string }) {
  const id = useId();
  const { choice, available, loading, saving } = state;
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
              disabled={!available || saving}
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
      {state.restart && choice && (
        <p className="field-hint settings-hint network-restart" role="status" data-testid={`${testId}-restart`}>
          {NetworkText.restart(choice)}
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
    </div>
  );
}
