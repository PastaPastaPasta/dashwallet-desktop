// Settings ▸ CoinJoin (U-06 D11, QT-046): dash-qt's Options ▸ CoinJoin for the network. The two
// boxes apply as they change, the numbers with one Save (checked against `coinjoin_limits`
// first), Restore defaults after a confirmation. The ranges and defaults are the engine's.

import { useId, useState, type FormEvent } from "react";
import type { CoinJoinLimits } from "../../api/generated/CoinJoinLimits";
import type { CoinJoinSettings } from "../../api/generated/CoinJoinSettings";
import { DashButton } from "../../components/basics";
import { QuestionSheet } from "../../components/sheet";
import { checkSettings, CoinJoinText, formOf, settingsFailureText, type FormField, type SettingsForm } from "../../coinjoin/model";
import { useCoinJoinLimits, useCoinJoinSettings, useSaveCoinJoinSettings } from "../../coinjoin/queries";
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
  /** A box as clicked, until the engine has answered (U-04 D36). */
  const [turning, setTurning] = useState<Partial<CoinJoinSettings>>({});
  const shown = { ...current, ...turning };
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
      setProblem(settingsFailureText(e));
      return false;
    } finally {
      setSaving(false);
    }
  };

  const submit = (e: FormEvent) => {
    e.preventDefault();
    // Parallel sessions is off without multi-session: what was typed there is not saved.
    const checked = checkSettings(current.multiSession ? form : { ...form, maxSessions: String(current.maxSessions) }, current, limits);
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
  };

  const restore = () =>
    void write(limits.defaults).then((ok) => {
      if (!ok) return;
      setEdits({});
      setWrong(null);
      setDone(CoinJoinText.saved);
    });

  const box = (testId: string, label: string, hint: string, checked: boolean, patch: (on: boolean) => Partial<CoinJoinSettings>) => (
    <div className="coinjoin-setting">
      <label className="checkbox-row">
        <input
          type="checkbox"
          data-testid={testId}
          checked={checked}
          disabled={saving}
          aria-describedby={`${id}-${testId}`}
          onChange={(e) => {
            const p = patch(e.target.checked);
            setTurning(p);
            void write({ ...current, ...p }).finally(() => setTurning({}));
          }}
        />
        {label}
      </label>
      <Hint id={`${id}-${testId}`}>{hint}</Hint>
    </div>
  );

  return (
    <>
      <SettingsCard title="CoinJoin" testId="settings-coinjoin">
        {box("settings-coinjoin-enabled", CoinJoinText.enableFeatures, CoinJoinText.enableFeaturesTip, shown.enabled, (enabled) => ({ enabled }))}
        {box("settings-coinjoin-multisession", CoinJoinText.multiSession, CoinJoinText.multiSessionTip, shown.multiSession, (multiSession) => ({ multiSession }))}
        <Hint>{CoinJoinText.persists}</Hint>
      </SettingsCard>
      <SettingsCard title="Mixing" testId="settings-coinjoin-mixing">
        <form className="coinjoin-settings-form" noValidate onSubmit={submit}>
          {FIELDS.map(({ field, label, hint }) => (
            <div className="form-field" key={field}>
              <label htmlFor={`${id}-${field}`}>{label}</label>
              <input
                id={`${id}-${field}`}
                data-testid={`settings-coinjoin-${field}`}
                inputMode="numeric"
                autoComplete="off"
                value={form[field]}
                disabled={field === "maxSessions" && !shown.multiSession}
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
        <QuestionSheet
          title={CoinJoinText.restoreTitle}
          text={CoinJoinText.restoreMessage}
          confirm="Restore"
          testId="settings-coinjoin-restore-sheet"
          confirmTestId="settings-coinjoin-restore-confirm"
          onConfirm={restore}
          onClose={() => setRestoring(false)}
        />
      )}
    </>
  );
}
