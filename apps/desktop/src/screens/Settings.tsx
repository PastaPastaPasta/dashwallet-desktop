// Settings, until U-04 builds it (U-02 D14): a Wallet card with the menus' Add Wallet… and
// Show Recovery Phrase…, so the flows have a way in that a pointer (and WebDriver) can reach.

import { ui } from "../app/ui";
import { DashButton } from "../components/basics";

export function SettingsPage() {
  return (
    <main className="page" data-testid="page-settings">
      <div className="page-column">
        <header className="top-intro">
          <h1 className="top-intro-title">Settings</h1>
        </header>
        <section className="card-l1 settings-card" aria-labelledby="settings-wallet">
          <h2 id="settings-wallet" className="settings-card-title">
            Wallet
          </h2>
          <DashButton variant="plainBlue" data-testid="settings-add-wallet" onClick={() => ui.set({ onboarding: true })}>
            Add wallet…
          </DashButton>
          <DashButton variant="plainBlue" data-testid="settings-recovery-phrase" onClick={() => ui.set({ dialog: "recoveryPhrase" })}>
            Show recovery phrase…
          </DashButton>
        </section>
        <p className="placeholder card-l1" data-testid="placeholder-settings">
          The other settings are not available yet.
        </p>
      </div>
    </main>
  );
}
