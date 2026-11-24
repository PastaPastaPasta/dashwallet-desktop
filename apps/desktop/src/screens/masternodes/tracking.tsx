// Masternodes ▸ My masternodes: Track (MG-10 §1.3), Add keys and Keys you added (§1.4, MG-00 D13
// amended for 1.0: a tracked masternode's voting key is added here too, and then it votes), and
// Stop tracking. A pasted key goes to `mn_attach_key` and is cleared from state when the call ends
// (D9); it is never echoed in an error.

import { useEffect, useId, useState } from "react";
import type { KeyCheckName } from "../../api/generated/KeyCheckName";
import type { ListedMasternode } from "../../api/generated/ListedMasternode";
import type { MasternodeRow } from "../../api/generated/MasternodeRow";
import { isAppError } from "../../api/backend";
import { useBackend, useLockStatus } from "../../api/queries";
import { ui } from "../../app/ui";
import { DashButton } from "../../components/basics";
import { Sheet } from "../../components/sheet";
import { shortAddress } from "../../governance/model";
import { useT } from "../../i18n";
import {
  addKeyWaitText,
  type AttachedKeyLine,
  attachedLines,
  canAttach,
  isVaultLocked,
  keyErrorText,
  keyResultText,
  keyRoles,
  kindText,
  type KeyRoleView,
  latestRequest,
  MnText,
  mnErrorText,
  rowName,
  trackErrorText,
  untrackKeysText,
} from "../../masternodes/model";
import { useKeysChanged, useMasternodesRefresh } from "../../masternodes/queries";

/** mn-engine's `MAX_MASTERNODE_LABEL`. */
export const MAX_LABEL = 64;

export function SheetFooter({ onCancel, children }: { onCancel: () => void; children?: React.ReactNode }) {
  const t = useT();
  return (
    <footer className="mg-sheet-footer">
      <span />
      <span className="mg-actions">
        <DashButton type="button" variant="tintedGray" size="medium" onClick={onCancel}>
          {t("masternodes.myMasternodes.cancel")}
        </DashButton>
        {children}
      </span>
    </footer>
  );
}

export function TrackSheet({ rows, onClose, onTracked }: { rows: MasternodeRow[]; onClose: () => void; onTracked: (proTxHash: string) => void }) {
  const t = useT();
  const backend = useBackend();
  const refresh = useMasternodesRefresh();
  const id = useId();
  const [query, setQuery] = useState("");
  const [found, setFound] = useState<ListedMasternode[] | null>(null);
  const [picked, setPicked] = useState<string | null>(null);
  const [label, setLabel] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const nameOf = (hash: string) => {
    const r = rows.find((x) => x.proTxHash === hash);
    return r ? rowName(r) : null;
  };
  const [searches] = useState(latestRequest);
  const find = async () => {
    const ticket = searches.start();
    setBusy(true);
    setError(null);
    try {
      const hits = await backend.mnFind(query.trim());
      if (!searches.isCurrent(ticket)) return;
      setFound(hits);
      setPicked(hits.find((h) => !h.tracked)?.proTxHash ?? null);
      if (hits.length === 0) setError(t("masternodes.myMasternodes.track.none"));
    } catch (e) {
      if (!searches.isCurrent(ticket)) return;
      setFound(null);
      setError(trackErrorText(e, null));
    } finally {
      setBusy(false);
    }
  };
  const track = async () => {
    if (!picked) return;
    setBusy(true);
    setError(null);
    try {
      await backend.mnTrack(picked, label.trim() || null);
      await refresh();
      onTracked(picked);
    } catch (e) {
      setError(trackErrorText(e, nameOf(picked)));
      setBusy(false);
    }
  };
  return (
    <Sheet title={t("masternodes.myMasternodes.track.title")} width="medium" testId="mn-track-sheet" onClose={onClose}>
      <form
        className="mg-sheet-body"
        onSubmit={(e) => {
          e.preventDefault();
          void (found && picked ? track() : find());
        }}
      >
        <label className={`mg-field ${error && !found?.length ? "has-error" : ""}`}>
          {t("masternodes.myMasternodes.track.query")}
          <span className="mg-field-row">
            <input
              className="mono"
              data-testid="mn-track-query"
              value={query}
              autoComplete="off"
              spellCheck={false}
              data-autofocus
              aria-describedby={`${id}-help`}
              onChange={(e) => {
                searches.drop();
                setQuery(e.target.value);
                setFound(null);
                setPicked(null);
              }}
            />
            <DashButton type="button" variant="tintedBlue" size="small" data-testid="mn-track-find" disabled={busy || query.trim() === ""} onClick={() => void find()}>
              {t("masternodes.myMasternodes.track.find")}
            </DashButton>
          </span>
          <span className="mg-field-help" id={`${id}-help`}>
            {t("masternodes.myMasternodes.track.queryHelp")}
          </span>
        </label>
        {found && found.length > 0 && (
          <fieldset className="mg-choices" disabled={busy} data-testid="mn-track-results">
            <legend className="mg-section-title">{t("masternodes.myMasternodes.track.results")}</legend>
            {found.map((l) => (
              <label key={l.proTxHash} className="mg-choice" data-testid={`mn-track-result-${l.proTxHash}`}>
                <input type="radio" name="mn-track-pick" checked={picked === l.proTxHash} disabled={l.tracked} onChange={() => setPicked(l.proTxHash)} />
                <span className="mg-choice-text">
                  <span>{t("masternodes.myMasternodes.rowSub", { kind: kindText(l.facts.kind), service: l.facts.service ?? t("masternodes.myMasternodes.notInList") })}</span>
                  <span className="mg-muted mg-small">
                    {l.tracked ? t("masternodes.myMasternodes.track.alreadyTracked") : t("masternodes.myMasternodes.track.votingAddress", { address: shortAddress(l.facts.votingAddress) })}
                  </span>
                </span>
              </label>
            ))}
          </fieldset>
        )}
        {found && picked && (
          <label className="mg-field">
            {t("masternodes.myMasternodes.track.label")}
            <input data-testid="mn-track-label" value={label} maxLength={MAX_LABEL} autoComplete="off" onChange={(e) => setLabel(e.target.value)} />
            <span className="mg-field-help">{t("masternodes.myMasternodes.track.labelHelp")}</span>
          </label>
        )}
        <p className="mg-field-error" role="alert" data-testid="mn-track-error">
          {error}
        </p>
        <SheetFooter onCancel={onClose}>
          <DashButton type="submit" variant="filledBlue" size="medium" data-testid="mn-track-submit" disabled={busy || !found || !picked}>
            {t("masternodes.myMasternodes.track.submit")}
          </DashButton>
        </SheetFooter>
      </form>
    </Sheet>
  );
}

/** `onUnlock`: close the sheet and open the app's unlock sheet, which isn't shown over another sheet. */
export function AddKeySheet({ row, onClose, onUnlock }: { row: MasternodeRow; onClose: () => void; onUnlock: () => void }) {
  const t = useT();
  const backend = useBackend();
  const refresh = useKeysChanged();
  const id = useId();
  const lock = useLockStatus();
  // The offer is made once the attached keys are read, then kept: a refusal re-reads them, and its reason
  // must stay on screen rather than the form going away under it.
  const live = keyRoles(row).at(0);
  const [offered, setOffered] = useState(live);
  useEffect(() => {
    if (!offered && live) setOffered(live);
  }, [offered, live]);
  const writable = lock.isSuccess && canAttach(lock.data.state);
  const [key, setKey] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<{ text: string; unlock: boolean } | null>(null);
  const [result, setResult] = useState<KeyCheckName | null>(null);
  const add = async (view: KeyRoleView) => {
    if (key.trim() === "") return setError({ text: t("masternodes.myMasternodes.keys.invalid"), unlock: false });
    setBusy(true);
    setError(null);
    const given = key;
    setKey("");
    try {
      const check = await backend.mnAttachKey(row.proTxHash, view.role, given);
      await refresh();
      setResult(check);
    } catch (e) {
      setError({ text: keyErrorText(e), unlock: isVaultLocked(e) });
      // A key was added meanwhile (another window): read it, so the pane shows it with its Remove.
      if (isAppError(e) && e.code === "masternodes.key_exists") void refresh();
    } finally {
      setBusy(false);
    }
  };
  return (
    <Sheet title={t("masternodes.myMasternodes.keys.title")} width="small" testId="mn-add-key-sheet" onClose={onClose}>
      {result !== null ? (
        <div className="mg-sheet-body">
          <p className="mg-notice mg-notice-info" role="status" data-testid="mn-add-key-result">
            {keyResultText(result)}
          </p>
          <footer className="mg-sheet-footer">
            <span />
            <DashButton variant="filledBlue" size="medium" data-testid="mn-add-key-done" data-autofocus onClick={onClose}>
              {t("masternodes.myMasternodes.keys.done")}
            </DashButton>
          </footer>
        </div>
      ) : writable && offered ? (
        <form
          className="mg-sheet-body"
          onSubmit={(e) => {
            e.preventDefault();
            void add(offered);
          }}
        >
          <p className="mg-muted mg-small mg-flush">{rowName(row)}</p>
          <div data-testid="mn-add-key-role">
            <h3 className="mg-section-title">{offered.title}</h3>
            <p className="mg-muted mg-small mg-flush">{offered.help}</p>
          </div>
          <label className={`mg-field ${error ? "has-error" : ""}`}>
            {t("masternodes.myMasternodes.keys.input")}
            <input
              className="mono"
              type="password"
              data-testid="mn-add-key-input"
              value={key}
              autoComplete="off"
              spellCheck={false}
              data-autofocus
              aria-invalid={!!error}
              aria-describedby={`${id}-help ${id}-error`}
              disabled={busy}
              onChange={(e) => setKey(e.target.value)}
            />
            <span className="mg-field-help" id={`${id}-help`}>
              {t("masternodes.myMasternodes.keys.inputHelp")}
            </span>
          </label>
          <p className="mg-small mg-notice-row" id={`${id}-error`}>
            <span className="mg-field-error" role="alert" data-testid="mn-add-key-error">
              {error?.text}
            </span>
            {error?.unlock && (
              <DashButton type="button" variant="tintedBlue" size="small" data-testid="mn-add-key-unlock"
                onClick={onUnlock}
              >
                {MnText.unlock}
              </DashButton>
            )}
          </p>
          <SheetFooter onCancel={onClose}>
            <DashButton type="submit" variant="filledBlue" size="medium" data-testid="mn-add-key-submit" disabled={busy}>
              {t("masternodes.myMasternodes.keys.submit")}
            </DashButton>
          </SheetFooter>
        </form>
      ) : (
        <div className="mg-sheet-body" data-testid="mn-add-key-unlock-first">
          {writable && (
            <p className="mg-small" role="status" data-testid="mn-add-key-wait">
              {addKeyWaitText(row)}
            </p>
          )}
          {lock.isSuccess && !writable && (
            <p className="mg-small mg-notice-row">
              <span>{t("masternodes.myMasternodes.keys.locked")}</span>
              <DashButton variant="tintedBlue" size="small" data-testid="mn-add-key-unlock" data-autofocus onClick={onUnlock}>
                {MnText.unlock}
              </DashButton>
            </p>
          )}
          <SheetFooter onCancel={onClose} />
        </div>
      )}
    </Sheet>
  );
}

/** The pane's "Keys you added" with Remove; "unlock to see" while the vault is locked. */
export function KeysYouAdded({ row }: { row: MasternodeRow }) {
  const t = useT();
  const [removing, setRemoving] = useState<AttachedKeyLine["role"] | null>(null);
  const lines = attachedLines(row);
  const removingLine = lines?.find((l) => l.role === removing);
  // The key went (another window, a lock): drop the confirmation rather than reopen it when the role is back.
  useEffect(() => {
    if (removing && !removingLine) setRemoving(null);
  }, [removing, removingLine]);
  if (lines === null)
    return (
      <p className="mg-small mg-muted mg-notice-row" data-testid="mn-keys-added-locked">
        <span>{t("masternodes.myMasternodes.keys.addedLocked")}</span>
        <DashButton variant="tintedBlue" size="small" data-testid="mn-keys-added-unlock" onClick={() => ui.set({ dialog: "unlock" })}>
          {MnText.unlock}
        </DashButton>
      </p>
    );
  if (lines.length === 0) return null;
  return (
    <div data-testid="mn-keys-added">
      <h3 className="mg-section-title">{t("masternodes.myMasternodes.keys.added")}</h3>
      <ul className="mg-keys-added">
        {lines.map((l) => (
          <li key={l.role} data-testid={`mn-key-added-${l.role}`}>
            <span>{l.text}</span>
            <DashButton variant="plainBlue" size="small" data-testid={`mn-key-remove-${l.role}`} onClick={() => setRemoving(l.role)}>
              {l.remove}
            </DashButton>
          </li>
        ))}
      </ul>
      {removingLine && <RemoveKeySheet row={row} line={removingLine} onClose={() => setRemoving(null)} />}
    </div>
  );
}

/** Remove always asks first: the private key is deleted for every wallet on this computer. */
function RemoveKeySheet({ row, line, onClose }: { row: MasternodeRow; line: AttachedKeyLine; onClose: () => void }) {
  const t = useT();
  const backend = useBackend();
  const refresh = useKeysChanged();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const remove = async () => {
    setBusy(true);
    setError(null);
    try {
      await backend.mnDetachKey(row.proTxHash, line.role);
      await refresh();
      onClose();
    } catch (e) {
      setError(mnErrorText(e));
      setBusy(false);
    }
  };
  return (
    <Sheet title={line.confirmTitle} width="small" testId="mn-remove-key-sheet" onClose={onClose}>
      <div className="mg-sheet-body">
        <p className="mg-flush">{t("masternodes.myMasternodes.keys.removeText")}</p>
        {error && (
          <p className="mg-field-error" role="alert" data-testid="mn-remove-key-error">
            {error}
          </p>
        )}
        <SheetFooter onCancel={onClose}>
          <DashButton variant="plainRed" size="medium" data-testid="mn-remove-key-confirm" disabled={busy} onClick={() => void remove()}>
            {t("masternodes.myMasternodes.keys.removeConfirm")}
          </DashButton>
        </SheetFooter>
      </div>
    </Sheet>
  );
}

export function UntrackSheet({ row, onClose }: { row: MasternodeRow; onClose: () => void }) {
  const t = useT();
  const backend = useBackend();
  const refresh = useKeysChanged();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const keysLine = untrackKeysText(row);
  const stop = async () => {
    setBusy(true);
    try {
      await backend.mnUntrack(row.proTxHash);
      await refresh();
      onClose();
    } catch (e) {
      setError(mnErrorText(e));
      setBusy(false);
    }
  };
  return (
    <Sheet title={t("masternodes.myMasternodes.untrack.title")} width="small" testId="mn-untrack-sheet" onClose={onClose}>
      <div className="mg-sheet-body">
        <p className="mg-flush">{t("masternodes.myMasternodes.untrack.text", { name: rowName(row) })}</p>
        {keysLine && (
          <p className="mg-flush" data-testid="mn-untrack-keys">
            {keysLine}
          </p>
        )}
        {error && (
          <p className="mg-field-error" role="alert" data-testid="mn-untrack-error">
            {error}
          </p>
        )}
        <SheetFooter onCancel={onClose}>
          <DashButton variant="plainRed" size="medium" data-testid="mn-untrack-submit" disabled={busy} onClick={() => void stop()}>
            {t("masternodes.myMasternodes.untrack.submit")}
          </DashButton>
        </SheetFooter>
      </div>
    </Sheet>
  );
}
