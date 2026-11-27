// Settings ▸ Advanced ▸ Connected apps (DashPay App Connect): lists each app approved for the
// current identity whose session keys are still enabled on Dash Platform, and lets the user
// disconnect an app after reviewing its Platform credit fee and entering their passphrase.

import { useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import type { ConnectedApp } from "../../api/generated/ConnectedApp";
import type { ConnectedAppKey } from "../../api/generated/ConnectedAppKey";
import type { IdentitySummary } from "../../api/generated/IdentitySummary";
import type { NetworkInfo } from "../../api/generated/NetworkInfo";
import { useBackend } from "../../api/queries";
import { DashButton } from "../../components/basics";
import { CopyIconButton } from "../../components/copy";
import { DetailRow } from "../../components/detail";
import { Sheet } from "../../components/sheet";
import { creditsDash } from "../../dashpay/connect";
import {
  ConnectedAppsModel,
  connectedAppsKeys,
  connectedKeyCritical,
  connectedKeyText,
  disconnectErrorText,
  formatConnectDate,
  useConnectedApps,
} from "../../dashpay/connectedApps";
import { creditCount } from "../../dashpay/profile";
import { useT } from "../../i18n";
import { useModel } from "../../onboarding/model";
import { GroupDetails } from "../dashpay/AppConnectSheet";
import { GrantSheet } from "../tools/common";
import { SettingsCard } from "./parts";

function KeyItem({ item, detailed }: { item: ConnectedAppKey; detailed: boolean }) {
  const t = useT();
  const { scope } = item;
  const critical = detailed ? connectedKeyCritical(item) : null;
  const contract = scope.kind === "contract" || scope.kind === "documentType" ? scope.contract : null;
  return (
    <li className="connected-app-key" data-testid="connected-app-key" data-key-id={item.keyId}>
      <div className="connected-app-key-head">
        <span className="identity-badge" data-testid="connected-app-key-id">
          {t("dashpay.connect.connected.keyId", { id: item.keyId })}
        </span>
        <span>{connectedKeyText(item)}</span>
      </div>
      {detailed && contract !== null && (
        <span className="connected-app-key-detail mono">
          <bdi dir="ltr">{contract}</bdi>
        </span>
      )}
      {detailed && scope.kind === "group" && <GroupDetails group={scope.group} />}
      {critical !== null && (
        <p className="sheet-warning connected-app-critical" role="note" data-testid="connected-app-critical">
          {critical}
        </p>
      )}
    </li>
  );
}

function AppRow({
  app,
  blockTimeMs,
  unitName,
  locale,
  busy,
  onDisconnect,
}: {
  app: ConnectedApp;
  blockTimeMs: number;
  unitName: string;
  locale?: string;
  busy: boolean;
  onDisconnect: () => void;
}) {
  const t = useT();
  const expiryDate = app.expiresAtMs !== null ? formatConnectDate(app.expiresAtMs, locale) : null;
  const expired = app.expiresAtMs !== null && blockTimeMs > 0 && app.expiresAtMs <= blockTimeMs;
  const approvedDate = formatConnectDate(app.approvedAtMs, locale);
  const limitValue = `${creditsDash(BigInt(app.fundingLimit))} ${unitName}`;
  return (
    <li className="connected-app-row" data-testid="connected-app-row" data-request={app.request}>
      <div className="connected-app-head">
        <div className="connected-app-title">
          <p className="connect-claim" data-testid="connected-app-claim">
            {t("dashpay.connect.app.claim", { label: app.label })}
          </p>
          <div className="connect-app-id">
            <span className="detail-label">{t("dashpay.connect.app.id")}</span>
            <span className="connect-app-id-value">
              <bdi dir="ltr" className="mono" data-testid="connected-app-id">
                {app.appId}
              </bdi>
              <CopyIconButton text={app.appId} label={t("dashpay.connect.app.copyId")} size={16} />
            </span>
          </div>
          {app.returnsTo !== null && (
            <p className="field-hint connected-app-returns" data-testid="connected-app-returns-to">
              {t("dashpay.connect.connected.returnsTo")} <bdi dir="ltr">{app.returnsTo}</bdi>
            </p>
          )}
        </div>
        <div className="connected-app-actions">
          <DashButton variant="tintedGray" size="small" data-testid="connected-app-disconnect" disabled={busy} onClick={onDisconnect}>
            {t(app.keys.length === 0 ? "dashpay.connect.connected.checkAttempt" : "dashpay.connect.connected.disconnect")}
          </DashButton>
        </div>
      </div>
      <dl className="detail-list connected-app-meta">
        <DetailRow label={t("dashpay.connect.connected.fundingLimit")} testId="connected-app-limit">
          <bdi className="mono">{limitValue}</bdi>
        </DetailRow>
        {expiryDate !== null && !expired && (
          <DetailRow label={t("dashpay.connect.connected.expiry")} testId="connected-app-expiry">
            {expiryDate}
          </DetailRow>
        )}
        <DetailRow label={t("dashpay.connect.connected.approved")} testId="connected-app-approved">
          {approvedDate}
        </DetailRow>
      </dl>
      {expiryDate !== null && expired && (
        <p className="field-hint" data-testid="connected-app-expiry">
          {t("dashpay.connect.connected.expiryPast", { date: expiryDate })}
        </p>
      )}
      {app.keys.length > 0 && (
        <div className="connected-app-keys-block">
          <p className="connect-asks-title">{t("dashpay.connect.connected.keys")}</p>
          <ul className="connected-app-keys" data-testid="connected-app-keys">
            {app.keys.map((k) => (
              <KeyItem key={k.keyId} item={k} detailed />
            ))}
          </ul>
        </div>
      )}
    </li>
  );
}

function DisconnectSheet({ model, username }: { model: ConnectedAppsModel; username: string | null }) {
  const t = useT();
  const app = model.target;
  if (!app) return null;
  const error = model.quoteError ?? model.disconnectError;
  const confirmationOnly = model.quote?.fee === "0" && model.quote.keys.length > 0;
  // Once the live quote resolves, render only the key IDs the quote will
  // actually disable (falling back to cached metadata for descriptions).
  const shownKeys =
    model.quote !== null
      ? model.quote.keys.map((keyId) => ({ keyId, item: app.keys.find((k) => k.keyId === keyId) ?? null }))
      : app.keys.map((item) => ({ keyId: item.keyId, item }));
  return (
    <Sheet
      title={t("dashpay.connect.connected.sheetTitle")}
      width="medium"
      testId="disconnect-sheet"
      closable={!model.disconnecting}
      onClose={() => model.closeDisconnect()}
    >
      <div className="connect-body" data-testid="disconnect-body">
        <div className="connect-app">
          <p className="connect-claim" data-testid="disconnect-claim">
            {t("dashpay.connect.app.claim", { label: app.label })}
          </p>
          <div className="connect-app-id">
            <span className="detail-label">{t("dashpay.connect.app.id")}</span>
            <span className="connect-app-id-value">
              <bdi dir="ltr" className="mono" data-testid="disconnect-app-id">
                {app.appId}
              </bdi>
            </span>
          </div>
          {app.returnsTo !== null && (
            <p className="field-hint" data-testid="disconnect-returns-to">
              {t("dashpay.connect.connected.returnsTo")} <bdi dir="ltr">{app.returnsTo}</bdi>
            </p>
          )}
        </div>
        <p className="sheet-text" data-testid="disconnect-explanation">
          {username !== null
            ? t("dashpay.connect.connected.sheetBody", { who: `@${username}` })
            : t("dashpay.connect.connected.sheetBodySelf")}
        </p>
        {shownKeys.length > 0 && (
          <div className="connected-app-keys-block">
            <p className="connect-asks-title">{t("dashpay.connect.connected.keysLabel")}</p>
            <ul className="connected-app-keys" data-testid="disconnect-keys">
              {shownKeys.map(({ keyId, item }) =>
                item !== null ? (
                  <KeyItem key={keyId} item={item} detailed={false} />
                ) : (
                  <li key={keyId} className="connected-app-key" data-testid="connected-app-key" data-key-id={keyId}>
                    <div className="connected-app-key-head">
                      <span className="identity-badge" data-testid="connected-app-key-id">
                        {t("dashpay.connect.connected.keyId", { id: keyId })}
                      </span>
                    </div>
                  </li>
                ),
              )}
            </ul>
          </div>
        )}
        {model.disconnecting ? (
          <div className="connect-loading" role="status" data-testid="disconnect-busy">
            <span className="spinner" aria-hidden="true" />
            <span>{t("dashpay.connect.connected.busy")}</span>
          </div>
        ) : (
          <>
            {error !== null && (
              <p className="form-problem" role="alert" data-testid="disconnect-error">
                {disconnectErrorText(error)}
              </p>
            )}
            {!confirmationOnly && model.quote?.priorAttemptConflict && (
              <p className="form-problem" role="alert" data-testid="disconnect-prior-charge">
                {model.quote.priorAttemptDebit !== null
                  ? t("dashpay.connect.connected.priorChargeKnown", { amount: creditCount(model.quote.priorAttemptDebit) })
                  : t("dashpay.connect.connected.priorChargePossible")}
              </p>
            )}
            {(error === null || model.requoted || model.quoting) && (
              <p className="sheet-text" role="status" data-testid="disconnect-fee">
                {confirmationOnly ? (
                  t("dashpay.connect.connected.checkAttemptBody")
                ) : model.quote !== null ? (
                  t("dashpay.connect.connected.fee", { amount: creditCount(model.quote.fee) })
                ) : (
                  model.quoting && <span className="spinner" aria-hidden="true" />
                )}
              </p>
            )}
          </>
        )}
        <div className="sheet-actions">
          <DashButton
            variant="tintedGray"
            size="large"
            data-testid="disconnect-cancel"
            disabled={model.disconnecting}
            onClick={() => model.closeDisconnect()}
          >
            {t("dashpay.connect.connected.cancel")}
          </DashButton>
          <DashButton
            variant="filledBlue"
            size="large"
            data-testid="disconnect-confirm"
            disabled={model.disconnecting || model.quoting || model.quote === null}
            onClick={() => void model.confirmDisconnect()}
          >
            {t(confirmationOnly ? "dashpay.connect.connected.checkAttempt" : "dashpay.connect.connected.disconnect")}
          </DashButton>
        </div>
      </div>
    </Sheet>
  );
}

/** Settings ▸ Advanced card listing the current (main) identity's connected apps. */
export function ConnectedAppsCard({
  walletId,
  network,
  identity,
}: {
  walletId: string;
  network: NetworkInfo;
  identity: IdentitySummary;
}) {
  const t = useT();
  const b = useBackend();
  const client = useQueryClient();
  const mainnet = network.kind === "mainnet";
  const query = useConnectedApps(walletId, identity.identity, !mainnet);
  const [model] = useState(
    () =>
      new ConnectedAppsModel(b, walletId, () => {
        void client.invalidateQueries({ queryKey: connectedAppsKeys.list(walletId, identity.identity) });
      }),
  );
  useModel(model);
  const username = identity.mainName ?? identity.names[0] ?? null;

  return (
    <SettingsCard title={t("dashpay.connect.connected.title")} testId="connected-apps-card">
      {mainnet ? (
        <p className="field-hint settings-hint" data-testid="connected-apps-mainnet-off">
          {t("dashpay.connect.err.mainnetOff")}
        </p>
      ) : (
        <>
          <p className="field-hint settings-hint" data-testid="connected-apps-intro">
            {username !== null
              ? t("dashpay.connect.connected.intro", { who: `@${username}` })
              : t("dashpay.connect.connected.introSelf")}
          </p>
          <div className="identities-actions">
            <DashButton
              variant="tintedGray"
              size="small"
              data-testid="connected-apps-refresh"
              disabled={query.isFetching || model.disconnecting}
              onClick={() => void query.refetch()}
            >
              {query.isFetching ? t("dashpay.connect.connected.checking") : t("dashpay.connect.connected.refresh")}
            </DashButton>
          </div>
          {model.disconnectedLabel !== null && model.lastDone !== null && (
            <p
              className={model.lastDone?.responseMayHaveLanded ? "form-problem" : "form-ok"}
              role={model.lastDone?.responseMayHaveLanded ? "alert" : "status"}
              data-testid="connected-apps-done"
            >
              {model.lastDone.keysRemaining > 0
                ? t("dashpay.connect.connected.disconnectedPartial", {
                    disabled: model.lastDone.keysDisabled.length,
                    total: model.lastDone.keysDisabled.length + model.lastDone.keysRemaining,
                  })
                : t(model.lastDone.responseMayHaveLanded
                    ? "dashpay.connect.connected.disconnectedResponsePending"
                    : "dashpay.connect.connected.disconnected", { label: model.disconnectedLabel })}
              {model.lastDone.keysRemaining > 0 && model.lastDone.responseMayHaveLanded && (
                <> {t("dashpay.connect.connected.responsePending")}</>
              )}
            </p>
          )}
          {query.isPending ? (
            <p className="placeholder-note" role="status" data-testid="connected-apps-loading">
              {t("dashpay.connect.connected.loading")}
            </p>
          ) : query.isError ? (
            <div className="identities-empty" data-testid="connected-apps-error">
              <p className="form-problem" role="alert">
                {t("dashpay.connect.connected.loadError")}
              </p>
              <DashButton variant="tintedGray" size="small" data-testid="connected-apps-retry" onClick={() => void query.refetch()}>
                {t("dashpay.connect.connected.tryAgain")}
              </DashButton>
            </div>
          ) : query.data.apps.length === 0 ? (
            <div className="identities-empty" data-testid="connected-apps-empty">
              <p>{t("dashpay.connect.connected.empty")}</p>
              <p className="field-hint">{t("dashpay.connect.connected.emptySub")}</p>
            </div>
          ) : (
            <ul className="connected-apps-list" data-testid="connected-apps-list">
              {query.data.apps.map((app) => (
                <AppRow
                  key={app.request}
                  app={app}
                  blockTimeMs={query.data.blockTimeMs}
                  unitName={network.unitName}
                  locale={b.locale}
                  busy={model.disconnecting}
                  onDisconnect={() => void model.openDisconnect(app)}
                />
              ))}
            </ul>
          )}
          <DisconnectSheet model={model} username={username} />
          <GrantSheet grants={model.grants} />
        </>
      )}
    </SettingsCard>
  );
}
