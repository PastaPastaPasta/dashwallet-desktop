// Share my username (PRODUCT #4, #12): Add contact's My QR in a sheet, its code large enough to
// scan across a table. The welcome and My Profile open it.

import { Sheet } from "../../components/sheet";
import { SHARE_TITLE } from "../../dashpay/registration";
import { MyQrTab } from "../contacts/AddContact";

export const SHARE_QR_SIZE = 280;

export function ShareSheet({
  walletId,
  identity,
  testId,
  returnFocus,
  onClose,
}: {
  walletId: string;
  identity: string;
  testId: string;
  returnFocus?: () => HTMLElement | null;
  onClose: () => void;
}) {
  return (
    <Sheet title={SHARE_TITLE} width="small" testId={testId} returnFocus={returnFocus} onClose={onClose}>
      <MyQrTab walletId={walletId} identity={identity} qrSize={SHARE_QR_SIZE} />
    </Sheet>
  );
}
