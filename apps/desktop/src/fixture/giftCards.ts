// dw-fixture-module: never in a production build (vite.config.ts, noFixtureModules).
import type { AppError } from "../api/generated/AppError";
import type { GiftAccount } from "../api/generated/GiftAccount";
import type { GiftOrder } from "../api/generated/GiftOrder";
import type { Handlers } from "./handlers";
import type { World } from "./scenarios";

const ADDRESS = "Xs6DNcUzKPYSvDDxpwFdDpGk4kNqhnUQ1T";
const BRANDS = [{ id: "45", name: "Fixture Groceries" }, { id: "88", name: "Fixture Books" }];
const fail = (message: string): AppError => ({ code: "invalid_argument", message, data: null });
const states = new WeakMap<World, { account: GiftAccount; orders: (GiftOrder & { walletId: string; paid: boolean })[]; drafts: Map<number, string> }>();

function giftCards(world: World) {
  if (world.launch.network.kind !== "mainnet" || !world.settings.ui.giftCardsNetworks.includes("mainnet")) {
    throw fail("Enable experimental gift cards on mainnet first.");
  }
  let state = states.get(world);
  if (!state) {
    state = { account: { kind: "signedOut" }, orders: [], drafts: new Map() };
    states.set(world, state);
  }
  return state;
}

function signedIn(world: World) {
  const state = giftCards(world);
  if (state.account.kind !== "signedIn") throw fail("Sign in to PiggyCards first.");
  return state;
}

export function giftCardsHandlers(send: Handlers): Handlers {
  return {
    gift_card_account: (_, { world }) => giftCards(world).account,
    gift_card_signup: ({ info }, { world }) => (giftCards(world).account = { kind: "codeSent", email: info.email }),
    gift_card_verify_email: ({ code }, { world }) => {
      const state = giftCards(world);
      if (state.account.kind !== "codeSent" || code !== "123456") throw fail("Check the email code.");
      return (state.account = { kind: "signedIn", email: state.account.email });
    },
    gift_card_sign_in: (_, { world }) => {
      const state = giftCards(world);
      if (state.account.kind !== "canSignIn") throw fail("Set up a PiggyCards account first.");
      return (state.account = { kind: "signedIn", email: state.account.email });
    },
    gift_card_sign_out: (_, { world }) => {
      const state = giftCards(world);
      return (state.account = state.account.kind === "signedIn" ? { kind: "canSignIn", email: state.account.email } : { kind: "signedOut" });
    },
    gift_card_brands: (_, { world }) => {
      signedIn(world);
      return BRANDS;
    },
    gift_card_products: ({ brand }, { world }) => {
      signedIn(world);
      const found = BRANDS.find((b) => b.id === brand);
      return found ? [{ id: Number(brand), name: found.name, values: [2500, 5000, 10000], min: null, max: null }] : [];
    },
    gift_card_orders: ({ walletId }, { world }) => signedIn(world).orders.filter((o) => o.walletId === walletId).slice().reverse(),
    gift_card_create_order: ({ walletId, brand, productId, cents }, { world }) => {
      const state = signedIn(world);
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
      const order = signedIn(world).orders.find((o) => o.id === orderId && o.walletId === walletId);
      if (!order) throw fail("Gift card order unavailable.");
      return {
        order,
        status: order.paid ? "complete" : "awaitingPayment",
        cards: order.paid ? [
          { name: order.name, code: "FIXTURE-GIFT-CODE", pin: "0000", claimHost: null },
          { name: "Fixture link-only card", code: null, pin: null, claimHost: "link-only.fixture.invalid" },
        ] : [],
      };
    },
    gift_card_open_claim: ({ walletId, orderId, card, host }, { world }) => {
      const order = signedIn(world).orders.find((o) => o.id === orderId && o.walletId === walletId);
      if (!order?.paid || card !== 1 || host !== "link-only.fixture.invalid") throw fail("The redemption link is unavailable.");
      return null;
    },
    gift_card_draft: ({ walletId, orderId }, ctx) => {
      const state = signedIn(ctx.world);
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
      const state = signedIn(world);
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
