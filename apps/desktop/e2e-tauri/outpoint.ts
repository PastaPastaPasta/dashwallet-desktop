export type Coin = { outpoint: { txid: string; vout: number }; address: string; amount: number };
export type TxOut = { value: number; scriptPubKey: { address?: string; addresses?: string[] } };

/** Why dashd's unspent output for `coin` is not the coin the wallet recorded: gone, another
 * amount, or paying another address. Signing for the address then would not prove the key
 * that holds the recorded coin. */
export function outpointProblem(coin: Coin, out: TxOut | null): string | undefined {
  const at = `${coin.outpoint.txid}:${coin.outpoint.vout}`;
  if (!out) return `dashd has no unspent output ${at}`;
  const duffs = Math.round(out.value * 100_000_000);
  if (duffs !== coin.amount) return `${at}: dashd has ${duffs} duffs, the wallet recorded ${coin.amount}`;
  const to = out.scriptPubKey.address ?? (out.scriptPubKey.addresses?.length === 1 ? out.scriptPubKey.addresses[0] : undefined);
  if (to !== coin.address) return `${at}: dashd pays ${to ?? "no single address"}, the wallet recorded ${coin.address}`;
  return undefined;
}
