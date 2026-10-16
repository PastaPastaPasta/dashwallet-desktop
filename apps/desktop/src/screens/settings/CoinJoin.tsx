// Settings ▸ CoinJoin (U-06 D11, QT-046): dash-qt's Options ▸ CoinJoin for the network. The two
// boxes apply as they change, the numbers with one Save (checked against `coinjoin_limits`
// first), Restore defaults after a confirmation. The ranges and defaults are the engine's.

import { useId, useState } from "react";
import type { CoinJoinLimits } from "../../api/generated/CoinJoinLimits";
import type { CoinJoinSettings } from "../../api/generated/CoinJoinSettings";
import { DashButton } from "../../components/basics";
import { Sheet } from "../../components/sheet";
import { checkSettings, CoinJoinText, formOf, type FormField, type SettingsForm } from "../../coinjoin/model";
import { useCoinJoinLimits, useCoinJoinSettings, useSaveCoinJoinSettings } from "../../coinjoin/queries";
import { failureText } from "../../onboarding/model";
import { Done, Hint, pendingText, Problem, SettingsCard } from "./parts";

const FIELDS: { field: FormField; label: string; hint?: string }[] = [
  { field: "rounds", label: CoinJoinText.mixingRounds, hint: CoinJoinText.mixingRoundsTip },
  { field: "targetAmountDash", label: CoinJoinText.targetBalance, hint: CoinJoinText.targetBalanceTip },
  { field: "maxSessions", label: CoinJoinText.parallelSessions, hint: CoinJoinText.parallelSessionsTip },
  { field: "denomsGoal", label: `${CoinJoinText.inputsPerDenomination}: ${CoinJoinText.denomsTarget}` },
  { field: "denomsHardCap", label: `${CoinJoinText.inputsPerDenomination}: ${CoinJoinText.denomsMaximum}`, hint: CoinJoinText.inputsPerDenominationTip },
];

export function CoinJoinTab() {
  const settings = useCoinJoinSettings();
  const limits = useCoinJoinLimits();
  if (!settings.data || !limits.data)
    return (
      <SettingsCard title="CoinJoin" testId="settings-coinjoin-loading">
        <Hint>{pendingText(settings.isError ? settings : limits)}</Hint>
      </SettingsCard>
    );
  return <CoinJoinForm current={settings.data} limits={limits.data} />;
}

function CoinJoinForm({ current, limits }: { current: CoinJoinSettings; limits: CoinJoinLimits }) {
  const save = useSaveCoinJoinSettings();
  const id = useId();
  /** The fields as typed; the rest show the saved values. */
  const [edits, setEdits] = useState<Partial<SettingsForm>>({});
  const [wrong, setWrong] = useState<FormField | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const [done, setDone] = useState<string | null>(null);
  /** One write at a time: a second answer could arrive first (U-04 D36). */
  const [saving, setSaving] = useState(false);
  const [restoring, setRestoring] = useState(false);
  const form = { ...formOf(current), ...edits };
  const changed = Object.keys(edits).length > 0;

  /** Resolves to whether the engine took `next`; a refusal shows its message. */
  const write = async (next: CoinJoinSettings) => {
    setSaving(true);
    setProblem(null);
    setDone(null);
    try {
      await save(next);
      return true;
    } catch (e) {
      setProblem(failureText(e));
      return false;
    } finally {
      setSaving(false);
    }
  };

  const box = (testId: string, label: string, hint: string, checked: boolean, patch: (on: boolean) => Partial<CoinJoinSettings>) => (
    <div className="coinjoin-setting">
      <label className="checkbox-row">
        <input
          type="checkbox"
          data-testid={testId}
          checked={checked}
          disabled={saving}
          aria-describedby={`${id}-${testId}`}
          onChange={(e) => void write({ ...current, ...patch(e.target.checked) })}
        />
        {label}
      </label>
      <Hint id={`${id}-${testId}`}>{hint}</Hint>
    </div>
  );

  return (
    <>
      <SettingsCard title="CoinJoin" testId="settings-coinjoin">
        {box("settings-coinjoin-enabled", CoinJoinText.enableFeatures, CoinJoinText.enableFeaturesTip, current.enabled, (enabled) => ({ enabled }))}
        {box("settings-coinjoin-multisession", CoinJoinText.multiSession, CoinJoinText.multiSessionTip, current.multiSession, (multiSession) => ({ multiSession }))}
        <Hint>{CoinJoinText.persists}</Hint>
      </SettingsCard>
      <SettingsCard title="Mixing" testId="settings-coinjoin-mixing">
        <form
          className="coinjoin-settings-form"
          noValidate
          onSubmit={(e) => {
            e.preventDefault();
            const checked = checkSettings(form, current, limits);
            if ("field" in checked) {
              setWrong(checked.field);
              setDone(null);
              return setProblem(checked.text);
            }
            void write(checked.ok).then((ok) => {
              if (!ok) return;
              setEdits({});
              setDone(CoinJoinText.saved);
            });
          }}
        >
          {FIELDS.map(({ field, label, hint }) => (
            <div className="form-field" key={field}>
              <label htmlFor={`${id}-${field}`}>{label}</label>
              <input
                id={`${id}-${field}`}
                data-testid={`settings-coinjoin-${field}`}
                inputMode="numeric"
                autoComplete="off"
                value={form[field]}
                disabled={field === "maxSessions" && !current.multiSession}
                aria-invalid={wrong === field ? true : undefined}
                aria-describedby={hint ? `${id}-${field}-hint` : undefined}
                onChange={(e) => {
                  setEdits((d) => ({ ...d, [field]: e.target.value }));
                  setWrong(null);
                  setProblem(null);
                  setDone(null);
                }}
              />
              {hint && <Hint id={`${id}-${field}-hint`}>{hint}</Hint>}
            </div>
          ))}
          <Problem text={problem} testId="settings-coinjoin-problem" />
          <Done text={done} testId="settings-coinjoin-done" />
          <div className="card-actions">
            <DashButton type="submit" variant="tintedBlue" data-testid="settings-coinjoin-save" disabled={!changed || saving}>
              Save
            </DashButton>
            {changed && (
              <DashButton
                variant="plainBlue"
                data-testid="settings-coinjoin-revert"
                onClick={() => {
                  setEdits({});
                  setWrong(null);
                  setProblem(null);
                }}
              >
                Cancel
              </DashButton>
            )}
          </div>
        </form>
        <DashButton variant="plainBlue" data-testid="settings-coinjoin-restore" disabled={saving} onClick={() => setRestoring(true)}>
          {CoinJoinText.restoreDefaults}
        </DashButton>
      </SettingsCard>
      {restoring && (
        <Sheet title={CoinJoinText.restoreTitle} width="small" testId="settings-coinjoin-restore-sheet" onClose={() => setRestoring(false)}>
          <p className="sheet-text">{CoinJoinText.restoreMessage}</p>
          <div className="sheet-actions">
            <DashButton variant="tintedGray" data-autofocus onClick={() => setRestoring(false)}>
              Cancel
            </DashButton>
            <DashButton
              variant="filledBlue"
              data-testid="settings-coinjoin-restore-confirm"
              onClick={() => {
                setRestoring(false);
                void write(limits.defaults).then((ok) => {
                  if (!ok) return;
                  setEdits({});
                  setWrong(null);
                  setDone(CoinJoinText.saved);
                });
              }}
            >
              Restore
            </DashButton>
          </div>
        </Sheet>
      )}
    </>
  );
}
