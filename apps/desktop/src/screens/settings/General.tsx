// Settings ▸ General (QT-135…141, QT-075, IOS-104…106): the display settings (saved as they
// change, D2), dust protection, the window's network (D6), the rows that come later (D7) and
// Reset Options (D8).

import { useQueryClient } from "@tanstack/react-query";
import { useId, useMemo, useState } from "react";
import type { ThemeName } from "../../api/generated/ThemeName";
import type { UiSettings } from "../../api/generated/UiSettings";
import { keys, useBackend, useDustProtection, useNodeInfo } from "../../api/queries";
import { useLaunch } from "../../app/App";
import { DEFAULT_PREFS, useUi } from "../../app/ui";
import { DashButton } from "../../components/basics";
import { CopyValue, DetailRow } from "../../components/detail";
import { Sheet } from "../../components/sheet";
import { failureText } from "../../onboarding/model";
import { DEFAULT_DUST, DIGIT_CHOICES, dustThreshold, OptionsText, UNIT_CHOICES } from "../../settings/options";
import { savePrefs, settledPending } from "../../settings/prefs";
import { Done, Hint, NotYetRow, pendingText, Problem, SettingsCard } from "./parts";

const THEMES: { theme: ThemeName | null; label: string; id: string }[] = [
  { theme: null, label: "System", id: "system" },
  { theme: "light", label: "Light", id: "light" },
  { theme: "dark", label: "Dark", id: "dark" },
];

/** The display settings as the controls show them, and their save; the problem says when dw-app
 * could not write them. A change shows at once, while its save is out (D36). */
export function useSavePrefs() {
  const backend = useBackend();
  const launch = useLaunch();
  const saved = useUi((s) => s.prefs);
  const [pending, setPending] = useState<Partial<UiSettings>>({});
  const [problem, setProblem] = useState<string | null>(null);
  /** Resolves to whether dw-app wrote the change; a failure shows `problem`. */
  const save = (patch: Partial<UiSettings>) => {
    setPending((p) => ({ ...p, ...patch }));
    return savePrefs(backend, launch, patch)
      .then(
        () => {
          setProblem(null);
          return true;
        },
        () => {
          setProblem(OptionsText.settingsNotSaved);
          return false;
        },
      )
      .finally(() => setPending((p) => settledPending(p, patch)));
  };
  const prefs = useMemo(() => ({ ...saved, ...pending }), [saved, pending]);
  return { prefs, save, problem };
}

export function GeneralTab() {
  const launch = useLaunch();
  const { prefs, save, problem } = useSavePrefs();
  const [resetting, setResetting] = useState(false);
  const [resetDone, setResetDone] = useState(false);
  const id = useId();
  return (
    <>
      <SettingsCard title="Display" testId="settings-display">
        <Problem text={problem} testId="settings-not-saved" />
        <div className="settings-row">
          <span className="settings-row-title" id={`${id}-theme`}>
            Appearance
          </span>
          <div className="segmented" role="group" aria-labelledby={`${id}-theme`}>
            {THEMES.map((t) => (
              <button
                key={t.id}
                type="button"
                className="segment"
                data-testid={`settings-theme-${t.id}`}
                aria-pressed={prefs.theme === t.theme}
                onClick={() => void save({ theme: t.theme })}
              >
                {t.label}
              </button>
            ))}
          </div>
        </div>
        {launch.theme !== null && <Hint>This window was started with --theme {launch.theme}, which wins until the next start without it.</Hint>}
        <div className="settings-row">
          <span className="settings-row-title" id={`${id}-unit`}>
            Unit to show amounts in
          </span>
          <div className="segmented" role="group" aria-labelledby={`${id}-unit`}>
            {UNIT_CHOICES.map((u) => (
              <button
                key={u.unit}
                type="button"
                className="segment"
                data-testid={`settings-unit-${u.unit}`}
                aria-pressed={prefs.unit === u.unit}
                onClick={() => void save({ unit: u.unit })}
              >
                {u.label}
              </button>
            ))}
          </div>
        </div>
        <div className="settings-row">
          <label className="settings-row-title" htmlFor={`${id}-digits`}>
            Decimal digits on Home
          </label>
          <select
            id={`${id}-digits`}
            className="settings-select"
            data-testid="settings-digits"
            value={prefs.decimalDigits}
            onChange={(e) => void save({ decimalDigits: Number(e.target.value) })}
          >
            {DIGIT_CHOICES.map((d) => (
              <option key={d} value={d}>
                {d}
              </option>
            ))}
          </select>
        </div>
        <label className="checkbox-row">
          <input
            type="checkbox"
            data-testid="settings-subtract-fee"
            checked={prefs.subtractFeeByDefault}
            onChange={(e) => void save({ subtractFeeByDefault: e.target.checked })}
          />
          Subtract the fee from the amount by default
        </label>
      </SettingsCard>
      <DustCard />
      <NetworkCard />
      <SettingsCard title="Coming later" testId="settings-not-yet">
        <NotYetRow testId="settings-language" title="User interface language" why="Dash Wallet is in English only until the translations (MP-14)." />
        <NotYetRow testId="settings-currency" title="Local currency" why="exchange rates come with MP-01." />
        <NotYetRow testId="settings-notifications" title="Notifications" why="DashPay notifications come with DP2-09, Windows notifications with W-02." />
        <NotYetRow testId="settings-start-on-login" title="Start Dash Wallet on system login" why="it comes with the installers (W-02, PK)." />
        <NotYetRow testId="settings-proxy" title="Connect through a SOCKS5 proxy or Tor" why="the engine has no proxy setting yet." />
      </SettingsCard>
      <SettingsCard title="Not used by this wallet" testId="settings-spv-footnote">
        <Hint>Dash Wallet does not keep the whole blockchain, so these Dash Core options do not apply:</Hint>
        <ul className="settings-list">
          {OptionsText.spvFootnote.map((o) => (
            <li key={o}>{o}</li>
          ))}
        </ul>
      </SettingsCard>
      <SettingsCard title="Reset" testId="settings-reset-card">
        <Done text={resetDone ? OptionsText.resetDone : null} testId="settings-reset-done" />
        <DashButton variant="plainRed" data-testid="settings-reset" onClick={() => setResetting(true)}>
          Reset options…
        </DashButton>
      </SettingsCard>
      {resetting && (
        <Sheet title={OptionsText.resetTitle} width="small" testId="settings-reset-sheet" onClose={() => setResetting(false)}>
          <p className="sheet-text">{OptionsText.resetMessage}</p>
          <Problem text={problem} testId="settings-reset-error" />
          <div className="sheet-actions">
            <DashButton variant="tintedGray" data-autofocus onClick={() => setResetting(false)}>
              Cancel
            </DashButton>
            <DashButton
              variant="filledBlue"
              data-testid="settings-reset-confirm"
              // Reset Options (QT-141, D8): every display setting back to its default.
              onClick={() =>
                // A failed save keeps the sheet open with the problem.
                void save(DEFAULT_PREFS).then((saved) => {
                  if (!saved) return;
                  setResetting(false);
                  setResetDone(true);
                })
              }
            >
              Reset
            </DashButton>
          </div>
        </Sheet>
      )}
    </>
  );
}

/** Dust attack protection (QT-075): the engine's, saved with its own button. */
function DustCard() {
  const backend = useBackend();
  const client = useQueryClient();
  const dust = useDustProtection();
  const [text, setText] = useState<string | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  /** The box as clicked, until the engine has answered (D36). */
  const [turning, setTurning] = useState<boolean | null>(null);
  const id = useId();
  if (dust.isPending) return null;
  const current = dust.data ?? null;
  const on = turning ?? current !== null;
  const shown = text ?? String(current ?? DEFAULT_DUST);
  const apply = async (threshold: number | null) => {
    setProblem(null);
    try {
      await backend.setDustProtection(threshold);
      setText(null);
      await client.invalidateQueries({ queryKey: keys.dust });
    } catch (e) {
      setProblem(failureText(e));
    }
  };
  return (
    <SettingsCard title="Dust attack protection" testId="settings-dust">
      {dust.isError && <Problem text={failureText(dust.error)} testId="settings-dust-error" />}
      <label className="checkbox-row">
        <input
          type="checkbox"
          data-testid="settings-dust-on"
          checked={on}
          onChange={(e) => {
            const want = e.target.checked;
            const t = dustThreshold(shown);
            setTurning(want);
            void apply(want ? (typeof t === "number" ? t : DEFAULT_DUST) : null).finally(() => setTurning(null));
          }}
        />
        Hide received coins at or under a threshold
      </label>
      <form
        className="settings-inline-form"
        onSubmit={(e) => {
          e.preventDefault();
          const t = dustThreshold(shown);
          if (typeof t === "string") return setProblem(t);
          void apply(t);
        }}
      >
        <div className="form-field">
          <label htmlFor={`${id}-dust`}>Threshold (duffs)</label>
          <input
            id={`${id}-dust`}
            data-testid="settings-dust-threshold"
            inputMode="numeric"
            autoComplete="off"
            value={shown}
            disabled={!on}
            aria-invalid={problem ? true : undefined}
            onChange={(e) => {
              setText(e.target.value);
              setProblem(null);
            }}
          />
        </div>
        <DashButton type="submit" variant="tintedBlue" data-testid="settings-dust-save" disabled={!on || text === null}>
          Save
        </DashButton>
      </form>
      <Problem text={problem} testId="settings-dust-problem" />
    </SettingsCard>
  );
}

/** The window's network and data directory; switching is U-04b (D6). */
function NetworkCard() {
  const { network } = useLaunch();
  const node = useNodeInfo();
  return (
    <SettingsCard title="Network" testId="settings-network">
      <dl className="detail-list settings-details">
        <DetailRow label="Network" testId="settings-network-name">
          {network.kind === "devnet" ? network.dirName : network.kind[0].toUpperCase() + network.kind.slice(1)}
        </DetailRow>
        <DetailRow label="Data directory" testId="settings-data-dir">
          {node.data ? <CopyValue value={node.data.dataDir} label="data directory" mono /> : pendingText(node)}
        </DetailRow>
      </dl>
      <NotYetRow
        testId="settings-switch-network"
        title="Switch network"
        why="it comes with U-04b. To open another network, start Dash Wallet with --network testnet, devnet-<name> or regtest."
      />
    </SettingsCard>
  );
}
