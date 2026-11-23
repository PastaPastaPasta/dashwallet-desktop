// My masternodes ▸ a shared masternode's pane section (`sh-detail`, MG-10 §2.4, §2.8 Shown and
// Validity, S6): the share bar and legend from EP 5, labelled from the registration session when the
// wallet holds it, your payouts as far as this wallet knows, your refund, the early period, the
// stored standbys and the eligibility line. Read-only: the maintenance actions come with a later
// slice and are absent until then (D14).

import type { ReactNode } from "react";
import type { MasternodeRow } from "../../api/generated/MasternodeRow";
import type { ShareFact } from "../../api/generated/ShareFact";
import type { Standby } from "../../api/generated/Standby";
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
  standbyLines,
  standbySource,
  type LegendEntry,
} from "../../masternodes/shared";

const P = "masternodes.shared.detail";
const strong = (c: ReactNode) => <strong>{c}</strong>;
const mono = (a: string) => <span className="mono">{shortAddress(a)}</span>;

export function SharedSection({ walletId, row, shown }: { walletId: string; row: MasternodeRow; shown: Display }) {
  const t = useT();
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
      <Standbys standbys={standby.data ?? []} fullValidNow={s.fullValidNow} legend={legend} shown={shown} />
      <p className="mg-small mg-muted" data-testid="sh-detail-backups">
        {t(`${P}.backups`)}
      </p>
      {line && (
        <p className="mg-notice mg-notice-info" data-testid={`sh-detail-eligibility-${s.eligibility}`}>
          {line}
        </p>
      )}
    </div>
  );
}

const labelOf = (legend: LegendEntry[], index: number) => legend.find((l) => l.index === index)?.label ?? "";

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

function Standbys({ standbys, fullValidNow, legend, shown }: { standbys: Standby[]; fullValidNow: boolean; legend: LegendEntry[]; shown: Display }) {
  const t = useT();
  if (standbys.length === 0) return null;
  const own = new Set(legend.filter((l) => l.mine).map((l) => l.index));
  const labelled = standbys.some((s) => !own.has(s.shareIndex));
  return (
    <div data-testid="sh-detail-standby">
      <h3 className="mg-section-title">{t(`${P}.standby.title`)}</h3>
      {standbys.map((s) => (
        <ul key={`${s.shareIndex}-${s.fee}`} className="sh-lines mg-small" data-testid={`sh-standby-${s.shareIndex}-${s.fee}`}>
          {labelled && <li className="sh-standby-share">{labelOf(legend, s.shareIndex)}</li>}
          {standbyLines(s, fullValidNow, (d) => shown.withUnit(d)).map((l) => (
            <li key={l.variant} data-testid={`sh-standby-${l.variant}`}>
              {l.text}
            </li>
          ))}
          <li className="mg-muted">{standbySource(s, (secs) => shortDate(secs))}</li>
        </ul>
      ))}
    </div>
  );
}
