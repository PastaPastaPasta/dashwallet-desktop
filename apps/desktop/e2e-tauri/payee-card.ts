// What the Pay form's payee card must show for a contact (DP3-06), as src/contacts/model.ts names
// it: the display name with @username under it, or the username alone when no display name is set.
// Node built-ins only, so vitest checks it.

export interface PayeeCard {
  name: string;
  sub: string | null;
}

/** Why `card` does not name the contact `username`; null when it does. With `displayName` given the
 * card must show exactly it over @username; without (not known, e.g. an older fixture.json) either
 * shape will do, each part still in its own place. */
export function payeeCardMismatch(card: PayeeCard, username: string, displayName?: string): string | null {
  const at = `@${username}`;
  if (displayName === undefined) {
    const named = card.name !== "" && card.name !== username && card.name !== at && card.sub === at;
    const bare = card.name === username && card.sub === null;
    return named || bare ? null : `the payee card shows ${JSON.stringify(card.name)} over ${JSON.stringify(card.sub)}, not a name over ${at} nor ${username} alone`;
  }
  const name = displayName.trim() || username;
  const sub = name === username ? null : at;
  const problems = [
    card.name === name ? null : `name ${JSON.stringify(card.name)}, not ${JSON.stringify(name)}`,
    card.sub === sub ? null : `subtitle ${JSON.stringify(card.sub)}, not ${JSON.stringify(sub)}`,
  ].filter(Boolean);
  return problems.length ? `the payee card shows ${problems.join(" and ")}` : null;
}
