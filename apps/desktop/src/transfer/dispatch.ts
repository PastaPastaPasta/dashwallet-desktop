import type { PendingDispatch } from "../api/generated/PendingDispatch";
import { showToast } from "../components/toast";
import { t } from "../i18n";

export function pendingDispatchLabel(state: PendingDispatch): string {
  return t(state === "broadcast" ? "transfer.status.sent" : "transfer.status.sending");
}

export function toastPendingTransfer(state: PendingDispatch | undefined, close: () => void) {
  close();
  showToast(pendingDispatchLabel(state ?? "queued"));
}

export function toastTransferFailure() {
  showToast(t("transfer.status.failed"));
}
