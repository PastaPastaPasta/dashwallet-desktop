// What every Tools tab shares (U-05): the passphrase sheet of a pending grant (D7, D15, D17),
// a card with a title, a "not yet" row naming its owning task (ROADMAP §0), and the wallet a tab
// needs.

import type { ReactNode } from "react";
import { PassphrasePrompt } from "../../components/passphrase";
import { Sheet } from "../../components/sheet";
import { useModel } from "../../onboarding/model";
import type { GrantModel } from "../../tools/grant";
import { EmptyState } from "../../components/history";
import { copyWithToast } from "../../components/toast";

const PURPOSE_TEXT: Record<string, string> = {
  spend: "Enter your passphrase to authorize this payment.",
  signMessage: "Enter your passphrase to sign the message.",
  platformOp: "Enter your passphrase to authorize this Dash Platform operation.",
};

/** The passphrase for the grant a tab is waiting on, with U-02's lockout policy. */
export function GrantSheet({ grants }: { grants: GrantModel }) {
  // The grant model passes on its lock model's changes.
  useModel(grants);
  const { lock } = grants;
  const request = grants.pending;
  if (!request) return null;
  return (
    <Sheet title="Authorize" width="small" testId="grant-sheet" onClose={() => grants.cancel()}>
      <p className="sheet-text">{PURPOSE_TEXT[request.purpose.kind] ?? "Enter your passphrase to continue."}</p>
      <PassphrasePrompt
        testId="grant-sheet"
        error={lock.message}
        busy={lock.working}
        disabled={lock.blocked}
        submitLabel="Authorize"
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
export function NotYet({ title, owner, children, testId }: { title: string; owner: string; children: ReactNode; testId: string }) {
  return (
    <section className="tool-card tool-not-yet card-l1" data-testid={testId} aria-label={title}>
      <h2 className="card-title">{title}</h2>
      <p className="placeholder-note">
        {children} <span className="tool-owner">Not yet ({owner}).</span>
      </p>
    </section>
  );
}

/** A tab that needs a wallet, on a network without one. */
export function NeedsWallet({ testId }: { testId: string }) {
  return <EmptyState testId={testId} title="No wallet on this network yet" message="Create or restore a wallet to use this tool." />;
}

/** An outcome line under an action: the result, or what went wrong. */
export function Outcome({ ok, text, testId }: { ok: boolean; text: string | null; testId: string }) {
  if (!text) return null;
  return (
    <p className={ok ? "form-ok" : "form-problem"} role={ok ? "status" : "alert"} data-testid={testId}>
      {text}
    </p>
  );
}

/** One item of a row's menu: a copy (`copy`; an empty text is disabled) or an action. */
export type MenuItem = { title: string; copy: string } | { title: string; run: () => void; disabled?: boolean };

/** A row's menu, as dash-qt's context menus: copies and actions behind one summary. */
export function RowMenu({ label, items, testId }: { label: string; items: MenuItem[]; testId: string }) {
  return (
    <details className="row-menu">
      <summary data-testid={testId}>{label}</summary>
      <div className="row-menu-items">
        {items.map((item) => (
          <button
            key={item.title}
            type="button"
            data-testid={`${testId}-${item.title.toLowerCase().replace(/\W+/g, "-")}`}
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
