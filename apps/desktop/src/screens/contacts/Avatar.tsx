// An avatar (DP2-07 D10): the initials of the name shown on the accent tint, or, when the engine
// hands one over (DP4-02), its verified, re-encoded PNG thumbnail. A picture that fails to load
// falls back to the initials. Avatars in every list are DP4-03's.

import { useState } from "react";

export function Avatar({ letters, size = "row", png = null }: { letters: string; size?: "row" | "pane"; png?: string | null }) {
  const [broken, setBroken] = useState<string | null>(null);
  const picture = png !== null && png !== broken;
  return (
    <span className={`contact-avatar contact-avatar-${size}`} aria-hidden="true" data-picture={picture ? "" : undefined}>
      {picture ? <img className="contact-avatar-img" src={`data:image/png;base64,${png}`} alt="" onError={() => setBroken(png)} /> : letters}
    </span>
  );
}
