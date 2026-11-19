// The update check (PK-03a): the setting (Settings ▸ General ▸ Privacy), the one check at launch,
// About's line and the status bar's chip. Notify only: nothing is downloaded or installed, and the
// release notes open on a URL dw-app builds, never one the renderer gives.

import { useQuery, useQueryClient, type QueryClient } from "@tanstack/react-query";
import { useEffect, useId, useRef, useState } from "react";
import type { Backend } from "../api/backend";
import { keys, useBackend } from "../api/queries";
import { failureText } from "../onboarding/model";
import { Hint, Problem } from "../screens/settings/parts";
import { availableVersion, checkDue } from "./model";

export function useUpdates() {
  const b = useBackend();
  return useQuery({ queryKey: keys.updates, queryFn: () => b.updateStatus() });
}

async function checkNow(b: Backend, client: QueryClient) {
  try {
    client.setQueryData(keys.updates, await b.checkForUpdate());
  } catch {
    // Turned off while it ran, or the engine is gone: the status says which.
    await client.invalidateQueries({ queryKey: keys.updates });
  }
}

/** Once per launch, with the check on: one request. */
export function useLaunchUpdateCheck() {
  const b = useBackend();
  const client = useQueryClient();
  const { data } = useUpdates();
  const started = useRef(false);
  useEffect(() => {
    if (started.current || !checkDue(data)) return;
    started.current = true;
    void checkNow(b, client);
  }, [b, client, data]);
}

/** Opens the available release's notes; resolves to an error text when it could not. */
export function useOpenReleaseNotes() {
  const b = useBackend();
  return () =>
    b.openReleaseNotes().then(
      () => null,
      () => "Could not open the release notes.",
    );
}

/** The status bar's quiet chip, while an update is available. */
export function UpdateStatusItem() {
  const { data } = useUpdates();
  const open = useOpenReleaseNotes();
  const version = availableVersion(data);
  if (!version) return null;
  return (
    <button type="button" className="status-button" data-testid="status-update" title={`Version ${version} is available: open its release notes`} onClick={() => void open()}>
      Update available
    </button>
  );
}

/** Settings ▸ General ▸ Privacy: "Check for updates", off by default. Not offered inside Flatpak. */
export function UpdateCheckRow() {
  const b = useBackend();
  const client = useQueryClient();
  const status = useUpdates();
  const [turning, setTurning] = useState<boolean | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const hint = useId();
  if (status.isPending || status.data?.supported === false) return null;
  const turn = async (on: boolean) => {
    setProblem(null);
    setTurning(on);
    try {
      const view = await b.setUpdateCheck(on);
      client.setQueryData(keys.updates, view);
      if (on) await checkNow(b, client);
    } catch (e) {
      setProblem(failureText(e));
    }
    setTurning(null);
  };
  return (
    <>
      {status.isError && <Problem text={failureText(status.error)} testId="settings-updates-error" />}
      <label className="checkbox-row">
        <input
          type="checkbox"
          data-testid="settings-update-check"
          checked={turning ?? status.data?.enabled === true}
          disabled={turning !== null}
          aria-describedby={hint}
          onChange={(e) => void turn(e.target.checked)}
        />
        Check for updates
      </label>
      <Hint id={hint}>
        Each time it starts, Dash Wallet asks GitHub whether a newer version was released and tells you in About and the status bar. GitHub sees your IP
        address. Nothing is downloaded or installed. Off by default.
      </Hint>
      <Problem text={problem} testId="settings-updates-problem" />
    </>
  );
}
