import type { Ref } from "react";
import { navigate, ui } from "../../app/ui";
import { DashButton } from "../../components/basics";
import { LoadingState } from "../../components/history";
import { useT } from "../../i18n";
import type { Banner } from "./contact";

/** Why the payee cannot be paid now, and what to do (D6, D7). */
export function PayBanner({
  banner,
  contact,
  checking,
  onCheck,
  retry,
  actionRef,
}: {
  banner: Banner;
  contact: string;
  checking: boolean;
  onCheck: () => void;
  retry: () => void;
  actionRef?: Ref<HTMLButtonElement>;
}) {
  const t = useT();
  if (banner.kind === "loading") return <LoadingState testId="pay-contact-loading" text={t("pay.contact.reading")} />;
  let action = null;
  switch (banner.kind) {
    case "readFailed":
      action = (
        <DashButton variant="tintedBlue" size="small" onClick={retry}>
          {t("pay.contact.retry")}
        </DashButton>
      );
      break;
    case "updating":
      action = (
        <DashButton ref={actionRef} variant="tintedBlue" size="small" data-testid="pay-banner-retry" disabled={checking} onClick={retry}>
          {t("pay.notice.tryAgain")}
        </DashButton>
      );
      break;
    case "enable":
      action = (
        <DashButton variant="tintedBlue" size="small" data-testid="pay-banner-contacts" onClick={() => navigate("contacts", { contact })}>
          {t("pay.contact.openInContacts")}
        </DashButton>
      );
      break;
    case "paymentLock":
      action = (
        <span className="add-contact-actions">
          {/* A payout's reference is no Core transaction: there is none to show yet. */}
          {banner.source === "core" && (
            <DashButton key="show" variant="tintedGray" size="small" data-testid="pay-banner-show" onClick={() => navigate("activity", { detail: banner.txid })}>
              {t("pay.contact.showTransaction")}
            </DashButton>
          )}
          <DashButton key="check" variant="tintedBlue" size="small" data-testid="pay-banner-check" disabled={checking} onClick={onCheck}>
            {checking ? t("pay.contact.checking") : t("pay.contact.checkAgain")}
          </DashButton>
        </span>
      );
      break;
    case "mixingOnly":
      action = (
        <DashButton variant="tintedBlue" size="small" data-testid="pay-banner-unlock" onClick={() => ui.set({ dialog: "unlock" })}>
          {t("pay.contact.unlock")}
        </DashButton>
      );
      break;
  }
  return (
    <div className={`contacts-hint card-l1 pay-banner is-${banner.kind}`} role="status" data-testid="pay-banner" data-kind={banner.kind}>
      <span data-testid="pay-banner-text">{banner.text}</span>
      {action}
    </div>
  );
}
