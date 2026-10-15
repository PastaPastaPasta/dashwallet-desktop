// A contact's avatar (DP2-07 D10): the initials of the name shown on the accent tint. Pictures come
// with DP4's avatar body (follow-up F2); `dashpay_avatar` is a stub at this base.

export function Avatar({ letters, size = "row" }: { letters: string; size?: "row" | "pane" }) {
  return (
    <span className={`contact-avatar contact-avatar-${size}`} aria-hidden="true">
      {letters}
    </span>
  );
}
