// Settings ▸ General (QT-135…141, QT-075, IOS-104…106): the display settings (saved as they
// change, D2), dust protection, the username in payment requests (DP3-03b), the window's network
// (D6), local currency (MP-01), the language (MP-14), the rows that come later (D7) and Reset
// Options (D8).

import { useQueryClient } from "@tanstack/react-query";
import { useId, useMemo, useState } from "react";
import type { NetworkInfo } from "../../api/generated/NetworkInfo";
import type { MasternodesItem } from "../../api/generated/MasternodesItem";
import type { ThemeName } from "../../api/generated/ThemeName";
import type { UiSettings } from "../../api/generated/UiSettings";
import { keys, useBackend, useDustProtection, useNodeInfo } from "../../api/queries";
import { useLaunch } from "../../app/App";
import { DEFAULT_PREFS, useUi } from "../../app/ui";
import { useTurnOnGovernance } from "../../governance/queries";
import { DashButton } from "../../components/basics";
import { NetworkPicker, useNetworkPicker } from "../../components/networkPicker";
import { networkName } from "../../network/choice";
import { CopyValue, DetailRow } from "../../components/detail";
import { QuestionSheet, Sheet } from "../../components/sheet";
import { byTag, LANGUAGES, systemLanguage, systemLanguages, useT } from "../../i18n";
import { unitDecimals } from "../../format/amount";
import { DECIMAL_PLACES, decimalPlaces, UNIT_OF } from "../../format/display";
import { failureText } from "../../onboarding/model";
import { DEFAULT_DUST, dustThreshold, OptionsText, UNIT_CHOICES } from "../../settings/options";
import { savePrefs, settledPending } from "../../settings/prefs";
import { SettingsDashPayCard } from "../dashpay/MyProfile";
import { Done, Hint, NotYetRow, pendingText, Problem, SettingsCard } from "./parts";
import { LocalCurrencyCard } from "./LocalCurrency";
import { PrivacyCard } from "./Privacy";

const THEMES: { theme: ThemeName | null; label: "themeSystem" | "themeLight" | "themeDark"; id: string }[] = [
  { theme: null, label: "themeSystem", id: "system" },
  { theme: "light", label: "themeLight", id: "light" },
  { theme: "dark", label: "themeDark", id: "dark" },
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

export function GeneralTab({ network }: { network: NetworkInfo }) {
  const t = useT();
  const launch = useLaunch();
  const { prefs, save, problem } = useSavePrefs();
  // Decimal places count in the chosen unit, at most its own decimals; a unit with one choice or
  // none (μDASH, duffs) has no row.
  const places = DECIMAL_PLACES.filter((n) => n <= unitDecimals(UNIT_OF[prefs.unit]));
  const [resetting, setResetting] = useState(false);
  const [resetDone, setResetDone] = useState(false);
  const id = useId();
  return (
    <>
      <SettingsCard title={t("settings.general.display")} testId="settings-display">
        <Problem text={problem} testId="settings-not-saved" />
        <div className="settings-row">
          <span className="settings-row-title" id={`${id}-theme`}>
            {t("settings.general.appearance")}
          </span>
          <div className="segmented" role="group" aria-labelledby={`${id}-theme`}>
            {THEMES.map((theme) => (
              <button
                key={theme.id}
                type="button"
                className="segment"
                data-testid={`settings-theme-${theme.id}`}
                aria-pressed={prefs.theme === theme.theme}
                onClick={() => void save({ theme: theme.theme })}
              >
                {t(`settings.general.${theme.label}`)}
              </button>
            ))}
          </div>
        </div>
        {launch.theme !== null && (
          <Hint>{t(launch.theme === "dark" ? "settings.general.themeForced.dark" : "settings.general.themeForced.light")}</Hint>
        )}
        <div className="settings-row">
          <span className="settings-row-title" id={`${id}-unit`}>
            {t("settings.general.unit")}
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
        {places.length > 1 && (
          <>
            <div className="settings-row">
              <span className="settings-row-title" id={`${id}-decimals`}>
                {t("settings.general.decimals")}
              </span>
              <div className="segmented" role="group" aria-labelledby={`${id}-decimals`} aria-describedby={`${id}-decimals-hint`}>
                {places.map((n) => (
                  <button
                    key={n}
                    type="button"
                    className="segment"
                    data-testid={`settings-decimals-${n}`}
                    aria-pressed={Math.min(decimalPlaces(prefs.decimalDigits), places[places.length - 1]) === n}
                    onClick={() => void save({ decimalDigits: n })}
                  >
                    {n}
                  </button>
                ))}
              </div>
            </div>
            <Hint id={`${id}-decimals-hint`}>{t("settings.general.decimalsHint")}</Hint>
          </>
        )}
        <label className="checkbox-row">
          <input
            type="checkbox"
            data-testid="settings-subtract-fee"
            checked={prefs.subtractFeeByDefault}
            onChange={(e) => void save({ subtractFeeByDefault: e.target.checked })}
          />
          {t("settings.general.subtractFee")}
        </label>
        <LanguageRow choice={prefs.language} save={save} />
        <ShowMasternodesRow choice={prefs.masternodesItem} save={save} />
      </SettingsCard>
      <LocalCurrencyCard />
      <SettingsCard title={t("giftCards.title")} testId="settings-gift-cards">
        <label className="checkbox-row">
          <input
            type="checkbox"
            checked={prefs.giftCardsNetworks.includes(network.dirName)}
            aria-describedby={`${id}-gift-cards-hint`}
            onChange={(e) => void save({
              giftCardsNetworks: e.target.checked
                ? [...prefs.giftCardsNetworks, network.dirName]
                : prefs.giftCardsNetworks.filter((name) => name !== network.dirName),
            })}
          />
          {t("giftCards.enable")}
        </label>
        <Hint id={`${id}-gift-cards-hint`}>{t("giftCards.settingHint")}</Hint>
      </SettingsCard>
      <DustCard />
      <PrivacyCard />
      <NetworkCard />
      <SettingsDashPayCard />
      <SettingsCard title={t("settings.general.comingSoon")} testId="settings-not-yet">
        <NotYetRow testId="settings-notifications" title={t("settings.general.notifications")} why={t("settings.general.notificationsWhy")} />
        <NotYetRow testId="settings-start-on-login" title={t("settings.general.startOnLogin")} why={t("settings.general.startOnLoginWhy")} />
        <NotYetRow testId="settings-proxy" title={t("settings.general.proxy")} why={t("settings.general.proxyWhy")} />
      </SettingsCard>
      <SettingsCard title={t("settings.general.resetCard")} testId="settings-reset-card">
        <Done text={resetDone ? OptionsText.resetDone : null} testId="settings-reset-done" />
        <DashButton variant="plainRed" data-testid="settings-reset" onClick={() => setResetting(true)}>
          {t("settings.general.resetOptions")}
        </DashButton>
      </SettingsCard>
      {resetting && (
        <Sheet title={OptionsText.resetTitle} width="small" testId="settings-reset-sheet" onClose={() => setResetting(false)}>
          <p className="sheet-text">{OptionsText.resetMessage}</p>
          <Problem text={problem} testId="settings-reset-error" />
          <div className="sheet-actions">
            <DashButton variant="tintedGray" data-autofocus onClick={() => setResetting(false)}>
              {t("common.cancel")}
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
              {t("settings.general.reset")}
            </DashButton>
          </div>
        </Sheet>
      )}
    </>
  );
}

/** Dust attack protection (QT-075): the engine's, saved with its own button. */
function DustCard() {
  const t = useT();
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
    <SettingsCard title={t("settings.general.dust")} testId="settings-dust">
      {dust.isError && <Problem text={failureText(dust.error)} testId="settings-dust-error" />}
      <label className="checkbox-row">
        <input
          type="checkbox"
          data-testid="settings-dust-on"
          checked={on}
          // One save at a time: a second click's answer could arrive first (D36).
          disabled={turning !== null}
          onChange={(e) => {
            const want = e.target.checked;
            const t = dustThreshold(shown);
            setTurning(want);
            void apply(want ? (typeof t === "number" ? t : DEFAULT_DUST) : null).finally(() => setTurning(null));
          }}
        />
        {t("settings.general.dustOn")}
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
          <label htmlFor={`${id}-dust`}>{t("settings.general.dustThreshold")}</label>
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
          {t("common.save")}
        </DashButton>
      </form>
      <Problem text={problem} testId="settings-dust-problem" />
    </SettingsCard>
  );
}

/** The window's network and data directory, and the switch to the other one (the 0.1 release
 * plan; D6's switch without a restart, row 74). */
function NetworkCard() {
  const t = useT();
  const { network } = useLaunch();
  const node = useNodeInfo();
  const picker = useNetworkPicker();
  return (
    <SettingsCard title={t("settings.general.network")} testId="settings-network">
      <dl className="detail-list settings-details">
        <DetailRow label={t("settings.general.network")} testId="settings-network-name">
          {network.kind === "devnet" ? network.dirName : networkName(network.kind)}
        </DetailRow>
        <DetailRow label={t("settings.general.dataDirectory")} testId="settings-data-dir">
          {node.data ? <CopyValue value={node.data.dataDir} action={t("settings.general.dataDirectoryCopy")} mono /> : pendingText(node)}
        </DetailRow>
      </dl>
      <NetworkPicker state={picker} testId="settings-network-choice" />
    </SettingsCard>
  );
}

/** Show Masternodes (MG-00 D2): the sidebar item whatever the wallet holds. Turning it on while
 * governance data is off asks once to load it too (MG-09 D6). */
/** The language of the app's own screens (MP-14): the system's, or one this build ships, each
 * named in its own language. */
function LanguageRow({ choice, save }: { choice: string | null; save: (patch: Partial<UiSettings>) => Promise<boolean> }) {
  const t = useT();
  const id = useId();
  const system = systemLanguage(systemLanguages());
  // A saved tag is bound to its option whatever its case; one this build does not ship gets a row
  // of its own, so it shows and choosing System default clears it.
  const shipped = choice === null ? undefined : byTag(choice);
  const value = choice === null ? "" : (shipped?.tag ?? choice);
  return (
    <>
      <div className="settings-row">
        <label className="settings-row-title" htmlFor={`${id}-language`}>
          {t("settings.general.language")}
        </label>
        <select
          id={`${id}-language`}
          className="settings-select"
          data-testid="settings-language"
          aria-describedby={`${id}-language-hint`}
          value={value}
          onChange={(e) => void save({ language: e.target.value || null })}
        >
          <option value="">{t("settings.general.languageSystem", { language: system.name })}</option>
          {choice !== null && !shipped && <option value={choice}>{t("settings.general.languageUnavailable", { tag: choice })}</option>}
          {LANGUAGES.map((l) => (
            <option key={l.tag} value={l.tag} lang={l.tag} dir={l.dir}>
              {l.name}
            </option>
          ))}
        </select>
      </div>
      <Hint id={`${id}-language-hint`}>{t("settings.general.languageHint")}</Hint>
    </>
  );
}

const MASTERNODES_ITEM: { item: MasternodesItem; label: "masternodesAuto" | "masternodesOn" | "masternodesOff" }[] = [
  { item: "auto", label: "masternodesAuto" },
  { item: "on", label: "masternodesOn" },
  { item: "off", label: "masternodesOff" },
];

function ShowMasternodesRow({ choice, save }: { choice: MasternodesItem; save: (patch: Partial<UiSettings>) => Promise<boolean> }) {
  const t = useT();
  const id = useId();
  const backend = useBackend();
  const { turnOn } = useTurnOnGovernance();
  const [ask, setAsk] = useState(false);
  const change = async (item: MasternodesItem) => {
    if (!(await save({ masternodesItem: item })) || item !== "on") return;
    if (!(await backend.governanceSyncEnabled().catch(() => true))) setAsk(true);
  };
  return (
    <>
      <div className="settings-row">
        <span className="settings-row-title" id={`${id}-masternodes`}>
          {t("settings.general.showMasternodes")}
        </span>
        <div className="segmented" role="group" aria-labelledby={`${id}-masternodes`} aria-describedby={`${id}-masternodes-hint`}>
          {MASTERNODES_ITEM.map((c) => (
            <button
              key={c.item}
              type="button"
              className="segment"
              data-testid={`settings-show-masternodes-${c.item}`}
              aria-pressed={choice === c.item}
              onClick={() => void change(c.item)}
            >
              {t(`settings.general.${c.label}`)}
            </button>
          ))}
        </div>
      </div>
      <Hint id={`${id}-masternodes-hint`}>{t("settings.general.showMasternodesHint")}</Hint>
      {ask && (
        <QuestionSheet
          title={t("settings.general.governanceAskTitle")}
          text={t("settings.general.governanceAskText")}
          confirm={t("settings.general.governanceTurnOn")}
          testId="settings-governance-ask"
          confirmTestId="settings-governance-turn-on"
          onConfirm={turnOn}
          onClose={() => setAsk(false)}
        />
      )}
    </>
  );
}
