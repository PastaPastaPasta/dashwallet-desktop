// My masternodes ▸ a shared masternode's pane section (`sh-detail`, MG-10 §2.4, §2.8, S6): the share
// bar and legend from EP 5, labelled from the registration session when the wallet holds it, your
// payouts as far as this wallet knows, your refund, the early period, the stored standbys and the
// eligibility line. While EP 5 reads `ok`, the maintenance actions: Dissolve…, Change keys together…,
// and each standby's Save a copy… and send. On mainnet no send shows; MG's line takes its place.

import { useState, type ReactNode } from "react";
import type { MasternodeRow } from "../../api/generated/MasternodeRow";
import type { NetworkInfo } from "../../api/generated/NetworkInfo";
import type { ShareFact } from "../../api/generated/ShareFact";
import type { WalletSummary } from "../../api/generated/WalletSummary";
import { isAppError } from "../../api/backend";
import { DashButton } from "../../components/basics";
import type { Display } from "../../format/display";
import { shortAddress, shortDate } from "../../governance/model";
import { useT } from "../../i18n";
import { MnText, mnErrorText } from "../../masternodes/model";
import { useSharedSessions, useSharedShares, useSharedStandby } from "../../masternodes/queries";
import {
  attemptLine,
  earlyEnd,
  eligibilityLine,
  payoutAddress,
  percentText,
  registrationSession,
  shareLegend,
  sharedSendsOn,
  labelOf,
  type LegendEntry,
} from "../../masternodes/shared";
import type { OpenSession } from "./Shared";
import { MainnetDissolveNote, MaintenanceSheets, Standbys, type MaintenanceCtx, type MaintenanceSheet } from "./SharedMaintenance";

const P = "masternodes.shared.detail";
const strong = (c: ReactNode) => <strong>{c}</strong>;
const mono = (a: string) => <span className="mono">{shortAddress(a)}</span>;

export function SharedSection({
  wallet,
  network,
  row,
  shown,
  operatorReward,
  onOpenSession,
}: {
  wallet: WalletSummary;
  network: NetworkInfo;
  row: MasternodeRow;
  shown: Display;
  operatorReward: number | null;
  onOpenSession: (s: OpenSession) => void;
}) {
  const t = useT();
  const walletId = wallet.walletId;
  const [sheet, setSheet] = useState<MaintenanceSheet | null>(null);
  const shares = useSharedShares(walletId, row.proTxHash);
  const sessions = useSharedSessions(walletId);
  const standby = useSharedStandby(walletId, row.proTxHash, shares.isSuccess);
  if (shares.isPending) return null;
  if (shares.isError) {
    if (isAppError(shares.error) && shares.error.code === "masternodes.shared_registration_unknown")
      return (
        <p className="mg-notice mg-notice-info" data-testid="sh-detail-unknown">
          {t(`${P}.unknown`)}
        </p>
      );
    return (
      <p className="mg-small mg-notice-row" data-testid="sh-detail-failed">
        <span className="mg-field-error">{t(`${P}.failed`, { reason: mnErrorText(shares.error) })}</span>
        <DashButton variant="tintedBlue" size="small" data-testid="sh-detail-retry" onClick={() => void shares.refetch()}>
          {MnText.tryAgain}
        </DashButton>
      </p>
    );
  }
  const s = shares.data;
  const session = registrationSession(sessions.data, row.proTxHash, s.session);
  const legend = shareLegend(s, session);
  const mine = s.shares.filter((f) => f.mine);
  const line = eligibilityLine(s.eligibility);
  const end = earlyEnd(s);
  const operatorBp = operatorReward ?? session?.terms.operatorReward ?? 0;
  const actions = s.eligibility === "ok" && !wallet.watchOnly && mine.length > 0;
  const c: MaintenanceCtx = { wallet, network, hash: row.proTxHash, shares: s, legend, session, standbys: standby.data ?? [], onOpenSession };
  return (
    <div className="sh-detail" data-testid="sh-detail">
      <div>
        <h3 className="mg-section-title">{t(`${P}.shares`)}</h3>
        <ShareBar legend={legend} shown={shown} />
        {!session && (
          <p className="mg-small mg-muted" data-testid="sh-detail-no-session">
            {t(`${P}.noSession`)}
          </p>
        )}
      </div>
      {mine.map((f) => (
        <Payouts key={f.index} fact={f} legend={legend} many={mine.length > 1} />
      ))}
      {operatorBp > 0 && (
        <p className="mg-small" data-testid="sh-detail-operator-share">
          {t(`${P}.operatorShare`, { pct: percentText(operatorBp / 100) })}
        </p>
      )}
      <dl className="mg-facts" data-testid="sh-detail-facts">
        {mine.map((f) => [
          <dt key={`refund-${f.index}-t`}>{mine.length > 1 ? t(`${P}.refundOf`, { label: labelOf(legend, f.index) }) : t(`${P}.refund`)}</dt>,
          <dd key={`refund-${f.index}-d`} data-testid={`sh-detail-refund-${f.index}`}>
            {mono(f.refundAddress)}
          </dd>,
        ])}
        <dt>{t(`${P}.early`)}</dt>
        <dd data-testid="sh-detail-early">{end === null ? t(`${P}.earlyNone`) : t(`${P}.earlyEnds`, { height: end })}</dd>
      </dl>
      <Standbys c={c} actions={actions} onSheet={setSheet} />
      <p className="mg-small mg-muted" data-testid="sh-detail-backups">
        {t(`${P}.backups`)}
      </p>
      {line && (
        <p className="mg-notice mg-notice-info" data-testid={`sh-detail-eligibility-${s.eligibility}`}>
          {line}
        </p>
      )}
      {actions && (
        <>
          {!sharedSendsOn(network) && <MainnetDissolveNote />}
          <span className="mg-actions" data-testid="sh-detail-actions">
            <DashButton variant="tintedBlue" size="small" data-testid="sh-dissolve-open" onClick={() => setSheet({ kind: "dissolve", segment: "alone" })}>
              {t(`${P}.dissolveAction`)}
            </DashButton>
            {sharedSendsOn(network) && (
              <DashButton variant="tintedBlue" size="small" data-testid="sh-keychange-open" onClick={() => setSheet({ kind: "keyChange" })}>
                {t(`${P}.changeKeysAction`)}
              </DashButton>
            )}
          </span>
        </>
      )}
      <MaintenanceSheets c={c} sheet={sheet} onSheet={setSheet} />
    </div>
  );
}

function ShareBar({ legend, shown }: { legend: LegendEntry[]; shown: Display }) {
  const t = useT();
  return (
    <>
      <div className="sh-sharebar" aria-hidden="true">
        {legend.map((l) => (
          <span key={l.index} className={l.mine ? "is-mine" : undefined} style={{ flexGrow: l.percent }} />
        ))}
      </div>
      <ul className="sh-legend" data-testid="sh-detail-legend">
        {legend.map((l) => (
          <li key={l.index} className={l.mine ? "is-mine" : undefined}>
            {t(l.mine ? `${P}.legendMine` : `${P}.legend`, { label: l.label, amount: shown.withUnit(l.amount) })}
          </li>
        ))}
      </ul>
    </>
  );
}

function Payouts({ fact, legend, many }: { fact: ShareFact; legend: LegendEntry[]; many: boolean }) {
  const t = useT();
  const share = legend.find((l) => l.index === fact.index);
  const changed = fact.reward.latestConfirmedLocal;
  const attempt = attemptLine(fact, shortAddress, (secs) => shortDate(secs));
  return (
    <div data-testid={`sh-detail-payouts-${fact.index}`}>
      <h3 className="mg-section-title">{many ? t(`${P}.payoutsOf`, { label: labelOf(legend, fact.index) }) : t(`${P}.payouts`)}</h3>
      <p className="mg-small" data-testid="sh-detail-headline">
        {t.rich(`${P}.headline`, { strong, share: percentText(share?.percent ?? 0), address: mono(payoutAddress(fact)) })}
      </p>
      <ul className="sh-lines mg-small">
        <li data-testid="sh-detail-at-registration">{t.rich(`${P}.atRegistration`, { address: mono(fact.reward.atRegistration) })}</li>
        {changed && <li data-testid="sh-detail-changed">{t.rich(`${P}.changed`, { address: mono(changed.address), height: changed.confirmedHeight })}</li>}
        {attempt && <li data-testid={`sh-detail-attempt-${fact.reward.latestAttempt?.state}`}>{attempt}</li>}
      </ul>
      <p className="mg-small mg-muted" data-testid="sh-detail-caveat">
        {t(`${P}.caveat`)}
      </p>
    </div>
  );
}
