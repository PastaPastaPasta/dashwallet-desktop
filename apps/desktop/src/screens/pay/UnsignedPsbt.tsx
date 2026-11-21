// Advanced: an unsigned transaction (P15). The payment as entered becomes a PSBT to sign
// elsewhere, reviewed on the confirm sheet and then opened in Tools ▸ PSBT to copy or save.
// Closed by default; open from the start for a watch-only wallet, whose only way to pay it is.

import { useEffect, useState, type Ref } from "react";
import { DashButton } from "../../components/basics";
import { useT } from "../../i18n";

export function UnsignedPsbt({
  watchOnly,
  why,
  onCreate,
  buttonRef,
  summaryRef,
}: {
  watchOnly: boolean;
  why: string | null;
  onCreate: () => void;
  buttonRef: Ref<HTMLButtonElement>;
  summaryRef: Ref<HTMLElement>;
}) {
  const t = useT();
  const [open, setOpen] = useState(watchOnly);
  useEffect(() => {
    if (watchOnly) setOpen(true);
  }, [watchOnly]);
  return (
    <details className="disclosure card-l1" data-testid="pay-advanced-psbt" open={open} onToggle={(e) => setOpen(e.currentTarget.open)}>
      <summary ref={summaryRef}>{t("pay.psbt.summary")}</summary>
      {open && (
        <div className="coin-control-body">
          <p className="placeholder-note" data-testid="pay-psbt-note">
            {watchOnly ? t("pay.psbt.watchOnly") : t("pay.psbt.note")}
          </p>
          <div className="tool-actions">
            <DashButton ref={buttonRef} variant={watchOnly ? "filledBlue" : "tintedBlue"} size="small" data-testid="pay-create-psbt" disabled={why !== null} why={why} onClick={onCreate}>
              {t("pay.psbt.create")}
            </DashButton>
          </div>
        </div>
      )}
    </details>
  );
}
