// Share my username (PRODUCT #4, #12): Add contact's My QR in a sheet, its code large enough to
// scan across a table, under whose it is. The welcome and My Profile open it.

import { Sheet } from "../../components/sheet";
import { SHARE_TITLE } from "../../dashpay/registration";
import { Avatar } from "../contacts/Avatar";
import { MyQrTab } from "../contacts/AddContact";

export const SHARE_QR_SIZE = 280;

/** Whose code the sheet shows: the display name if any, and the username without its "@". */
export interface ShareWho {
  name: string | null;
  username: string;
}

export function ShareSheet({
  walletId,
  identity,
  who,
  testId,
  returnFocus,
  onClose,
}: {
  walletId: string;
  identity: string;
  who: ShareWho | null;
  testId: string;
  returnFocus?: () => HTMLElement | null;
  onClose: () => void;
}) {
  const initials = who ? (Array.from(who.name ?? who.username)[0] ?? "?").toUpperCase() : "";
  return (
    <Sheet title={SHARE_TITLE} width="small" testId={testId} returnFocus={returnFocus} onClose={onClose}>
      {who && (
        <div className="share-who" data-testid="share-who">
          <Avatar letters={initials} identity={identity} own />
          <span className="share-who-text">
            <span className="share-who-name">{who.name ?? `@${who.username}`}</span>
            {who.name && <span className="share-who-username">@{who.username}</span>}
          </span>
        </div>
      )}
      <MyQrTab walletId={walletId} identity={identity} qrSize={SHARE_QR_SIZE} username={who?.username ?? null} />
    </Sheet>
  );
}
