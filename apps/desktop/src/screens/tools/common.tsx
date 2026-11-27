// What every Tools tab shares (U-05): the passphrase sheet of a pending grant (D7, D15, D17),
// a card with a title, a "not yet" row naming its owning task (ROADMAP §0), and the wallet a tab
// needs.

import { useEffect, useEffectEvent, type ReactNode } from "react";
import { useSyncStatus } from "../../api/queries";
import { ui } from "../../app/ui";
import { DashButton } from "../../components/basics";
import { PassphrasePrompt } from "../../components/passphrase";
import { Sheet } from "../../components/sheet";
import { messageTable, useT } from "../../i18n";
import { useModel } from "../../onboarding/model";
import { PAYMENT_PROMPT, type GrantModel, type GrantRequest } from "../../tools/grant";
import { EmptyState } from "../../components/history";
import { copyWithToast } from "../../components/toast";

const PURPOSE_TEXT: Readonly<Record<string, string>> = messageTable("tools.grant", ["spend", "signMessage", "platformOp", "platformSend", "appDisconnect"]);

/** The passphrase for the grant a tab is waiting on, with U-02's lockout policy. */
export function GrantSheet({ grants }: { grants: GrantModel }) {
  // The grant model passes on its lock model's changes.
  useModel(grants);
  const request = grants.pending;
  if (!request) return null;
  return <GrantSheetOpen grants={grants} request={request} />;
}

function GrantSheetOpen({ grants, request }: { grants: GrantModel; request: GrantRequest }) {
  const t = useT();
  const { lock } = grants;
  const prompt = grants.prompt === PAYMENT_PROMPT ? t("tools.grant.spend") : grants.prompt;
  return (
    // Not closed while the passphrase is checked: the grant it brings has to find its request.
    <Sheet title={t("tools.grant.title")} width="small" testId="grant-sheet" closable={!lock.working} onClose={() => grants.cancel()}>
      <p className="sheet-text">{prompt ?? PURPOSE_TEXT[request.purpose.kind] ?? t("tools.grant.continue")}</p>
      <PassphrasePrompt
        testId="grant-sheet"
        error={lock.message}
        busy={lock.working}
        disabled={lock.blocked}
        submitLabel={t("tools.grant.submit")}
        onSubmit={(p) => grants.submit(p)}
      />
    </Sheet>
  );
}

export function ToolCard({ title, testId, actions, children }: { title: string; testId: string; actions?: ReactNode; children?: ReactNode }) {
  return (
    <section className="tool-card card-l1" data-testid={testId} aria-label={title}>
      <header className="tool-card-header">
        <h2 className="card-title">{title}</h2>
        {actions && <div className="card-actions">{actions}</div>}
      </header>
      {children}
    </section>
  );
}

/** A row whose engine call or data source comes with a later task: said, not stubbed. */
export function NotYet({ title, children, testId }: { title: string; children: ReactNode; testId: string }) {
  const t = useT();
  return (
    <section className="tool-card tool-not-yet card-l1" data-testid={testId} aria-label={title}>
      <h2 className="card-title">{title}</h2>
      <p className="placeholder-note">
        {children} <span className="tool-owner">{t("tools.common.comingSoon")}</span>
      </p>
    </section>
  );
}

/** A tab that needs a wallet, on a network without one. */
export function NeedsWallet({ testId }: { testId: string }) {
  const t = useT();
  return (
    <EmptyState
      testId={testId}
      art="wallet"
      title={t("tools.common.noWalletTitle")}
      message={t("tools.common.noWalletMessage")}
      action={
        <DashButton variant="filledBlue" data-testid={`${testId}-create`} onClick={() => ui.set({ onboarding: true })}>
          {t("tools.common.createWallet")}
        </DashButton>
      }
    />
  );
}

/** Reads again whenever the sync status is read anew (every `sync` event refreshes it, D4). */
export function useReloadOnSync(reload: () => void) {
  const synced = useSyncStatus().dataUpdatedAt;
  const latest = useEffectEvent(reload);
  useEffect(() => latest(), [synced]);
}

/** An outcome line under an action: the result, or what went wrong. */
export function Outcome({ ok, text, testId, polite = false }: { ok: boolean; text: string | null; testId: string; polite?: boolean }) {
  return (
    <p className={`outcome ${ok ? "form-ok" : "form-problem"}`} role={ok || polite ? "status" : "alert"} data-testid={testId}>
      {text}
    </p>
  );
}

/** One item of a row's menu: a copy (`copy`; an empty text is disabled) or an action. */
export type MenuItem = { id: string; title: string; copy: string } | { id: string; title: string; run: () => void; disabled?: boolean };

/** A row's menu, as dash-qt's context menus: copies and actions behind one summary. */
/** `name` says whose menu it is to a screen reader ("Actions for …"), as rows look alike. */
export function RowMenu({ label, name, items, testId }: { label: string; name: string; items: MenuItem[]; testId: string }) {
  return (
    <details
      className="row-menu"
      onKeyDown={(e) => {
        // Escape closes the menu and gives focus back to its button, as a native menu does.
        if (e.key !== "Escape" || !e.currentTarget.open) return;
        e.preventDefault();
        e.stopPropagation();
        e.currentTarget.removeAttribute("open");
        e.currentTarget.querySelector("summary")?.focus();
      }}
    >
      <summary data-testid={testId} aria-label={name}>
        {label}
      </summary>
      <div className="row-menu-items">
        {items.map((item) => (
          <button
            key={item.id}
            type="button"
            data-testid={`${testId}-${item.id}`}
            disabled={"copy" in item ? item.copy === "" : item.disabled}
            onClick={(e) => {
              e.currentTarget.closest("details")?.removeAttribute("open");
              if ("copy" in item) void copyWithToast(item.copy);
              else item.run();
            }}
          >
            {item.title}
          </button>
        ))}
      </div>
    </details>
  );
}
