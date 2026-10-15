// Tools ▸ Peers (QT-147, IOS-023): what `peers()` reports, read again on every sync status
// change; a row opens its details; Change peers drops every connection and dials again.
// Disconnect, ban and unban are not built (D9, product scope).

import { useState } from "react";
import { useBackend } from "../../api/queries";
import { DashButton } from "../../components/basics";
import { DetailRow } from "../../components/detail";
import { LoadingState } from "../../components/history";
import { useModel } from "../../onboarding/model";
import { peerRow, PeersModel, PeersText, type PeerRow } from "../../tools/peers";
import { Outcome, ToolCard, useReloadOnSync } from "./common";

const COLUMNS: [keyof PeerRow, string, boolean][] = [
  ["address", "Address", false],
  ["direction", "Direction", false],
  ["userAgent", "User Agent", false],
  ["version", "Version", true],
  ["height", "Starting Block", true],
  ["ping", "Ping", true],
  ["connectedFor", "Connection Time", true],
  ["sent", "Sent", true],
  ["received", "Received", true],
];

export function PeersTab() {
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
        title="Peers"
        testId="peers-card"
        actions={
          <DashButton variant="tintedBlue" size="small" data-testid="peers-rotate" disabled={model.rotating} onClick={() => void model.rotate()}>
            {model.rotating ? "Changing peers…" : "Change peers"}
          </DashButton>
        }
      >
        <Outcome ok text={model.notice} testId="peers-notice" />
        <Outcome ok={false} text={model.error} testId="peers-error" />
        {model.peers === null ? (
          !model.error && <LoadingState testId="peers-loading" text="Reading the peers" />
        ) : rows.length === 0 ? (
          <p className="placeholder-note" data-testid="peers-none">
            {PeersText.none}
          </p>
        ) : (
          <div className="table-card">
            <table className="data-table peers-table" data-testid="peers-table">
              <caption className="sr-only">Connected peers</caption>
              <thead>
                <tr>
                  {COLUMNS.map(([key, title, num]) => (
                    <th key={key} scope="col" className={num ? "num" : undefined}>
                      {title}
                    </th>
                  ))}
                </tr>
              </thead>
              <tbody>
                {rows.map((r) => (
                  <tr key={r.address} data-testid={`peer-${r.address}`}>
                    {COLUMNS.map(([key, , num]) => (
                      <td key={key} className={num ? "num" : undefined}>
                        {key === "address" ? (
                          <button type="button" className="table-open mono" aria-expanded={r.address === open} onClick={() => setOpen(r.address === open ? null : r.address)}>
                            {r.address}
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
            {COLUMNS.slice(1).map(([key, title]) => (
              <DetailRow key={key} label={title}>
                {detail[key]}
              </DetailRow>
            ))}
          </dl>
        </ToolCard>
      )}
      <p className="placeholder-note" data-testid="peers-administration">
        Disconnecting, banning and unbanning peers are node administration, which stays with a Dash Core node.
      </p>
    </div>
  );
}
