// dw-fixture-module: never in a production build (vite.config.ts, noFixtureModules).
import type { AppError } from "../api/generated/AppError";
import type { GiftAccount } from "../api/generated/GiftAccount";
import type { GiftOrder } from "../api/generated/GiftOrder";
import type { GiftStoredAccount } from "../api/generated/GiftStoredAccount";
import type { Handlers } from "./handlers";
import type { World } from "./scenarios";

const ADDRESS = "Xs6DNcUzKPYSvDDxpwFdDpGk4kNqhnUQ1T";
const BRANDS = [{ id: "45", name: "Fixture Groceries" }, { id: "88", name: "Fixture Books" }];
const fail = (message: string): AppError => ({ code: "invalid_argument", message, data: null });
type Wallet = { account: GiftAccount; accounts: GiftStoredAccount[] };
const states = new WeakMap<World, { wallets: Map<string, Wallet>; orders: (GiftOrder & { walletId: string; paid: boolean })[]; drafts: Map<number, string> }>();

function giftCards(world: World) {
  if (world.launch.network.kind !== "mainnet" || !world.settings.ui.giftCardsNetworks.includes("mainnet")) {
    throw fail("Enable experimental gift cards on mainnet first.");
  }
  let state = states.get(world);
  if (!state) {
    state = { wallets: new Map(), orders: [], drafts: new Map() };
    states.set(world, state);
  }
  return state;
}

function walletOf(world: World, walletId: string): Wallet {
  const wallets = giftCards(world).wallets;
  let wallet = wallets.get(walletId);
  if (!wallet) {
    wallet = { account: { kind: "signedOut" }, accounts: [] };
    wallets.set(walletId, wallet);
  }
  return wallet;
}

function signedIn(world: World, walletId: string) {
  if (walletOf(world, walletId).account.kind !== "signedIn") throw fail("Sign in to PiggyCards first.");
  return giftCards(world);
}

function activate(wallet: Wallet, email: string): GiftAccount {
  wallet.accounts = [...wallet.accounts.filter((a) => a.email !== email).map((a) => ({ ...a, active: false })),
    { id: `fixture-${email}`, email, active: true }];
  return (wallet.account = { kind: "signedIn", email });
}

export function giftCardsHandlers(send: Handlers): Handlers {
  return {
    gift_card_account: ({ walletId }, { world }) => walletOf(world, walletId).account,
    gift_card_accounts: ({ walletId }, { world }) => walletOf(world, walletId).accounts,
    gift_card_signup: ({ walletId, info, termsAccepted }, { world }) => {
      if (!termsAccepted) throw fail("Accept the PiggyCards terms and conditions first.");
      return (walletOf(world, walletId).account = { kind: "codeSent", email: info.email });
    },
    gift_card_open_terms: () => null,
    gift_card_verify_email: ({ walletId, code }, { world }) => {
      const wallet = walletOf(world, walletId);
      if (wallet.account.kind !== "codeSent" || code !== "123456") throw fail("Check the email code.");
      return activate(wallet, wallet.account.email);
    },
    gift_card_sign_in: ({ walletId, account }, { world }) => {
      const wallet = walletOf(world, walletId);
      const chosen = account === null ? wallet.accounts.find((a) => a.active) : wallet.accounts.find((a) => a.id === account);
      if (!chosen) throw fail("Set up a PiggyCards account first.");
      return activate(wallet, chosen.email);
    },
    gift_card_sign_out: ({ walletId }, { world }) => {
      const wallet = walletOf(world, walletId);
      const active = wallet.accounts.find((a) => a.active);
      return (wallet.account = active ? { kind: "canSignIn", email: active.email } : { kind: "signedOut" });
    },
    gift_card_brands: ({ walletId }, { world }) => {
      signedIn(world, walletId);
      return BRANDS;
    },
    gift_card_products: ({ walletId, brand }, { world }) => {
      signedIn(world, walletId);
      const found = BRANDS.find((b) => b.id === brand);
      return found ? [{ id: Number(brand), name: found.name, values: [2500, 5000, 10000], min: null, max: null }] : [];
    },
    gift_card_orders: ({ walletId }, { world }) => signedIn(world, walletId).orders.filter((o) => o.walletId === walletId).slice().reverse(),
    gift_card_create_order: ({ walletId, brand, productId, cents }, { world }) => {
      const state = signedIn(world, walletId);
      const found = BRANDS.find((b) => b.id === brand);
      if (!found || productId !== Number(brand) || ![2500, 5000, 10000].includes(cents)) throw fail("This gift card value is unavailable.");
      const expiresAt = Math.floor(Date.now() / 1000) + 120;
      const order = {
        id: `fixture-order-${state.orders.length + 1}`, name: found.name, cents, expiresAt, txid: null,
        invoice: { address: ADDRESS, duffs: 12_345_678 }, walletId, paid: false,
      };
      state.orders.push(order);
      return order;
    },
    gift_card_details: ({ walletId, orderId }, { world }) => {
      const order = signedIn(world, walletId).orders.find((o) => o.id === orderId && o.walletId === walletId);
      if (!order) throw fail("Gift card order unavailable.");
      return {
        order,
        status: order.paid ? "complete" : "awaitingPayment",
        cards: order.paid ? [order.cents === 5000
          ? { name: order.name, code: null, pin: null, claimHost: "link-only.fixture.invalid" }
          : { name: order.name, code: "FIXTURE-GIFT-CODE", pin: "0000", claimHost: null }] : [],
      };
    },
    gift_card_resend: ({ walletId, orderId }, { world }) => {
      const order = signedIn(world, walletId).orders.find((o) => o.id === orderId && o.walletId === walletId);
      if (!order?.txid || order.paid || Date.now() >= order.expiresAt * 1000) throw fail("This order cannot be resent.");
      order.paid = true;
      return null;
    },
    gift_card_open_claim: ({ walletId, orderId, card, host }, { world }) => {
      const order = signedIn(world, walletId).orders.find((o) => o.id === orderId && o.walletId === walletId);
      if (!order?.paid || order.cents !== 5000 || card !== 0 || host !== "link-only.fixture.invalid") throw fail("The redemption link is unavailable.");
      world.giftClaimOpened = host;
      return null;
    },
    gift_card_draft: ({ walletId, orderId }, ctx) => {
      const state = signedIn(ctx.world, walletId);
      const order = state.orders.find((o) => o.id === orderId && o.walletId === walletId);
      if (!order || !order.invoice || order.txid || Date.now() >= order.expiresAt * 1000) throw fail("The gift card invoice is unavailable.");
      const handle = send.new_tx_draft!({ walletId }, ctx);
      state.drafts.set(handle, orderId);
      return handle;
    },
    draft_broadcast: (args, ctx) => {
      if (states.get(ctx.world)?.drafts.has(args.draft)) throw fail("This draft belongs to a different payment flow.");
      return send.draft_broadcast!(args, ctx);
    },
    draft_create_unsigned: (args, ctx) => {
      if (states.get(ctx.world)?.drafts.has(args.draft)) throw fail("This draft belongs to a different payment flow.");
      return send.draft_create_unsigned!(args, ctx);
    },
    gift_card_broadcast: ({ orderId, draft, prepared }, ctx) => {
      const { world } = ctx;
      const tx = world.send.prepared[prepared];
      const state = signedIn(world, tx?.walletId ?? "");
      const order = state.orders.find((o) => o.id === orderId && o.walletId === tx?.walletId);
      const invoice = order?.invoice;
      const recipients = tx?.summary.outputs.filter((o) => !o.isChange);
      if (!order || !invoice || !tx || state.drafts.get(draft) !== orderId || tx.draft !== draft || recipients?.length !== 1 ||
        recipients[0].address !== invoice.address || recipients[0].amount !== invoice.duffs ||
        recipients[0].isMine || Date.now() >= order.expiresAt * 1000) throw fail("The payment differs from the gift card invoice.");
      if (order.txid && order.txid !== tx.summary.txid) throw fail("A payment already started for this order.");
      order.txid = tx.summary.txid;
      try {
        const result = send.draft_broadcast!({ draft, prepared }, ctx);
        order.paid = true;
        return result;
      } catch (e) {
        if (!tx.handedOver) order.txid = null;
        throw e;
      }
    },
  };
}
