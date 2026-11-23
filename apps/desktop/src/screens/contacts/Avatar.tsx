// An avatar (DP2-07 D10, DP4-03 D12): the initials of the name shown on the accent tint, or the
// identity's verified, re-encoded PNG thumbnail when the engine hands one over (DP4-02). Every
// list uses it with the row's identity; the picture scope App provides says whose pictures may
// be read. A picture that fails to load falls back to the initials.

import { useState } from "react";
import { useAvatar } from "../../dashpay/profileQueries";

/** Rows and the chip read the 128 px thumbnail, panes the 256 px one. */
type Size = "row" | "pane" | "chip";

export function Avatar({
  letters,
  size = "row",
  identity = null,
  own = false,
  png,
}: {
  letters: string;
  size?: Size;
  /** Whose picture to show; without one, the initials alone. */
  identity?: string | null;
  /** One of the wallet's own identities (shown with "Load contact pictures" off too). */
  own?: boolean;
  /** A picture to show instead of the identity's (Edit Profile's new one). */
  png?: string | null;
}) {
  const read = useAvatar(png === undefined ? identity : null, size === "pane" ? "large" : "small", own);
  const shown = png === undefined ? read : png;
  const [broken, setBroken] = useState<string | null>(null);
  const picture = shown !== null && shown !== broken;
  const className = size === "chip" ? "identity-chip-avatar" : `contact-avatar contact-avatar-${size}`;
  return (
    <span className={className} aria-hidden="true" data-picture={picture ? "" : undefined}>
      {picture ? <img className="contact-avatar-img" src={`data:image/png;base64,${shown}`} alt="" onError={() => setBroken(shown)} /> : letters}
    </span>
  );
}
