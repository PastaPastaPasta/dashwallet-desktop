// BIP39 seed and hardened BIP32 derivation for the upgrade fixture's watch-only wallet: an account xpub of a
// public test vector no wallet in the fixture holds. Checked against the release's own account xpub before use.

import { createECDH, createHash, createHmac, pbkdf2Sync } from "node:crypto";

const N = BigInt("0xfffffffffffffffffffffffffffffffebaaedce6af48a03bbfd25e8cd0364141");
const B58 = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
const big = (b: Buffer) => BigInt(`0x${b.toString("hex")}`);
const bytes32 = (n: bigint) => Buffer.from(n.toString(16).padStart(64, "0"), "hex");
const sha256 = (b: Buffer) => createHash("sha256").update(b).digest();
const pub = (key: Buffer) => {
  const e = createECDH("secp256k1");
  e.setPrivateKey(key);
  return e.getPublicKey(null, "compressed") as Buffer;
};
function base58check(payload: Buffer) {
  const data = Buffer.concat([payload, sha256(sha256(payload)).subarray(0, 4)]);
  let n = big(data);
  let out = "";
  while (n > 0n) {
    out = B58[Number(n % 58n)]! + out;
    n /= 58n;
  }
  for (const b of data) {
    if (b !== 0) break;
    out = `1${out}`;
  }
  return out;
}

/** The testnet tpub at m/44'/1'/account' of a BIP39 mnemonic (hardened steps only). */
export function accountTpub(mnemonic: string, passphrase: string | null, account: number) {
  const seed = pbkdf2Sync(mnemonic.normalize("NFKD"), `mnemonic${passphrase ?? ""}`.normalize("NFKD"), 2048, 64, "sha512");
  let I = createHmac("sha512", "Bitcoin seed").update(seed).digest();
  let [key, chain] = [I.subarray(0, 32), I.subarray(32)];
  let parent = Buffer.alloc(4);
  const path = [44, 1, account].map((i) => i + 0x80000000);
  for (const index of path) {
    parent = createHash("ripemd160").update(sha256(pub(key))).digest().subarray(0, 4);
    const i = Buffer.alloc(4);
    i.writeUInt32BE(index);
    I = createHmac("sha512", chain).update(Buffer.concat([Buffer.alloc(1), key, i])).digest();
    key = bytes32((big(I.subarray(0, 32)) + big(key)) % N);
    chain = I.subarray(32);
  }
  const child = Buffer.alloc(4);
  child.writeUInt32BE(path.at(-1)!);
  return base58check(Buffer.concat([Buffer.from("043587cf", "hex"), Buffer.from([path.length]), parent, child, chain, pub(key)]));
}
