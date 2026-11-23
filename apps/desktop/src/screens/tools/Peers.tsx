// Tools ▸ Peers (QT-147, IOS-023): what `peers()` reports, read again on every sync status
// change; a row opens its details; Change peers drops every connection and dials again.
// Disconnect, ban and unban are not built (D9, product scope).

import { useState } from "react";
import { useBackend } from "../../api/queries";
import { DashButton } from "../../components/basics";
import { DetailRow } from "../../components/detail";
import { EMPTY } from "../../components/emptyStates";
import { EmptyState, LoadingState } from "../../components/history";
import { useT } from "../../i18n";
import { useModel } from "../../onboarding/model";
import { peerRow, PeersModel, type PeerRow } from "../../tools/peers";
import { Outcome, ToolCard, useReloadOnSync } from "./common";

const COLUMNS: [keyof PeerRow, boolean][] = [
  ["address", false],
  ["direction", false],
  ["userAgent", false],
  ["version", true],
  ["height", true],
  ["ping", true],
  ["connectedFor", true],
  ["sent", true],
  ["received", true],
];

export function PeersTab() {
  const t = useT();
  const b = useBackend();
  const [model] = useState(() => new PeersModel(b));
  useModel(model);
  const [open, setOpen] = useState<string | null>(null);
  useReloadOnSync(() => void model.load());
  const now = Math.floor(b.now().getTime() / 1000);
  const rows = (model.peers ?? []).map((p) => peerRow(p, now));
  const detail = rows.find((r) => r.address === open);
  return (
    <div className="tool-stack" data-testid="tools-peers">
      <ToolCard
        title={t("tools.peers.title")}
        testId="peers-card"
        actions={
          <DashButton variant="tintedBlue" size="small" data-testid="peers-rotate" disabled={model.rotating} onClick={() => void model.rotate()}>
            {model.rotating ? t("tools.peers.rotating") : t("tools.peers.rotate")}
          </DashButton>
        }
      >
        <Outcome ok text={model.notice} testId="peers-notice" />
        <Outcome ok={false} text={model.error} testId="peers-error" />
        {model.peers === null ? (
          !model.error && <LoadingState testId="peers-loading" text={t("tools.peers.loading")} />
        ) : rows.length === 0 ? (
          <EmptyState testId="peers-none" plain art={EMPTY.peers.art} title={EMPTY.peers.title} message={EMPTY.peers.message} />
        ) : (
          <div className="table-card">
            <table className="data-table peers-table" data-testid="peers-table">
              <caption className="sr-only">{t("tools.peers.tableCaption")}</caption>
              <thead>
                <tr>
                  {COLUMNS.map(([key, num]) => (
                    <th key={key} scope="col" className={num ? "num" : undefined}>
                      {t(`tools.peers.columns.${key}`)}
                    </th>
                  ))}
                </tr>
              </thead>
              <tbody>
                {rows.map((r) => (
                  <tr key={r.address} data-testid={`peer-${r.address}`}>
                    {COLUMNS.map(([key, num]) => (
                      <td key={key} className={num ? "num" : undefined}>
                        {key === "address" ? (
                          <button type="button" className="table-open mono" aria-expanded={r.address === open} onClick={() => setOpen(r.address === open ? null : r.address)}>
                            <bdi>{r.address}</bdi>
                          </button>
                        ) : (
                          r[key]
                        )}
                      </td>
                    ))}
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </ToolCard>
      {detail && (
        <ToolCard title={detail.address} testId="peer-detail">
          <dl className="detail-list">
            {COLUMNS.slice(1).map(([key]) => (
              <DetailRow key={key} label={t(`tools.peers.columns.${key}`)}>
                {detail[key]}
              </DetailRow>
            ))}
          </dl>
        </ToolCard>
      )}
      <p className="placeholder-note" data-testid="peers-administration">
        {t("tools.peers.administration")}
      </p>
    </div>
  );
}
