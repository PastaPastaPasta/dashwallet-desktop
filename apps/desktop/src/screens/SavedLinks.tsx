// "Links you opened" (DP5-01 D11): the links in the engine's link stash, offered once after
// onboarding, an unlock or a launch. Open takes a payment or contact link (once) and routes it
// as a clicked link; an invitation opens Join DashPay and stays saved. Remove forgets one; Later
// closes the sheet until the next offer.

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { errorMessage } from "../api/backend";
import { useBackend } from "../api/queries";
import { removeErrorText, savedLinkRows, SavedLinksText } from "../app/savedLinks";
import { applyLink } from "../app/shellBridge";
import { DashButton } from "../components/basics";
import { Sheet } from "../components/sheet";
import { showToast } from "../components/toast";

export const SAVED_LINKS_KEY = ["stashedLinks"] as const;

export function SavedLinksSheet({ onClose }: { onClose: () => void }) {
  const backend = useBackend();
  const client = useQueryClient();
  // Read afresh at every offer: links come and go without an event (intake, expiry).
  const links = useQuery({ queryKey: SAVED_LINKS_KEY, queryFn: () => backend.stashedLinks(), staleTime: 0, gcTime: 0 });
  const [busy, setBusy] = useState<string | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const rows = savedLinkRows(links.data ?? [], Date.now() / 1000);

  const act = async (id: string, what: () => Promise<void>, text: (e: unknown) => string = errorMessage) => {
    setBusy(id);
    setProblem(null);
    try {
      await what();
    } catch (e) {
      setProblem(text(e));
    } finally {
      setBusy(null);
      void client.invalidateQueries({ queryKey: SAVED_LINKS_KEY });
    }
  };
  const open = (id: string) =>
    act(id, async () => {
      const route = await backend.openStashedLink(id);
      onClose();
      if (route) applyLink(route);
      else showToast(SavedLinksText.gone);
    });
  const remove = (id: string) =>
    act(id, async () => {
      await backend.dismissStashedLink(id);
      if (rows.length <= 1) onClose();
    }, removeErrorText);

  return (
    <Sheet title={SavedLinksText.title} width="small" testId="saved-links" onClose={onClose}>
      <p className="sheet-text">{SavedLinksText.intro}</p>
      {links.isError ? (
        <p className="form-problem" role="alert" data-testid="saved-links-error">
          {SavedLinksText.loadFailed} {errorMessage(links.error)}
        </p>
      ) : (
        <ul className="saved-links" aria-label={SavedLinksText.title} data-testid="saved-links-list">
          {rows.map((row) => (
            <li key={row.id} className="saved-link" data-testid="saved-link" data-kind={row.invitation ? "invitation" : "link"}>
              <div className="saved-link-text">
                <span className="saved-link-title">{row.title}</span>
                <span className="saved-link-when">{row.when}</span>
              </div>
              <div className="saved-link-actions">
                <DashButton
                  variant="tintedGray"
                  size="small"
                  data-testid="saved-link-remove"
                  aria-label={`${SavedLinksText.remove}: ${row.title}`}
                  disabled={busy !== null}
                  onClick={() => void remove(row.id)}
                >
                  {SavedLinksText.remove}
                </DashButton>
                <DashButton
                  variant="filledBlue"
                  size="small"
                  data-testid="saved-link-open"
                  aria-label={`${SavedLinksText.open}: ${row.title}`}
                  disabled={busy !== null}
                  onClick={() => void open(row.id)}
                >
                  {SavedLinksText.open}
                </DashButton>
              </div>
            </li>
          ))}
        </ul>
      )}
      {problem && (
        <p className="form-problem" role="alert" data-testid="saved-links-problem">
          {problem}
        </p>
      )}
      <div className="sheet-actions">
        <DashButton variant="tintedGray" size="large" data-testid="saved-links-later" onClick={onClose}>
          {SavedLinksText.later}
        </DashButton>
      </div>
    </Sheet>
  );
}
