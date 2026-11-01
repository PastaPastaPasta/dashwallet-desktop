// The network choice (network/choice.ts): a segmented Mainnet / Testnet control, shown at
// onboarding and in Settings ▸ General. `useNetworkPicker` holds the choice and its save; the
// control and the notes under it are `NetworkPicker`.

import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useId } from "react";
import type { NetworkKind } from "../api/generated/NetworkKind";
import type { NetworkChoice } from "../api/pending";
import { errorMessage } from "../api/backend";
import { keys, useBackend, useNetworkChoice } from "../api/queries";
import { NETWORK_CHOICES, NetworkText, pinned, restartNeeded } from "../network/choice";

export function useNetworkPicker(current: NetworkKind) {
  const backend = useBackend();
  const client = useQueryClient();
  const query = useNetworkChoice();
  const save = useMutation({
    mutationFn: (choice: NetworkChoice) => backend.setNetworkChoice(choice),
    onSuccess: (_, choice) => client.setQueryData(keys.networkChoice, choice),
  });
  const choice = query.data ?? null;
  return {
    current,
    /** The saved choice; `null` while it is read, or when dw-app has no such command. */
    choice,
    available: choice !== null,
    loading: query.isPending,
    /** The saved choice is not the network this window runs on. */
    restart: restartNeeded(choice, current),
    pinned: pinned(current),
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
          {NetworkText.pinned(state.current)}
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
