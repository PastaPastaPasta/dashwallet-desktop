import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useRef, useState, type FormEvent } from "react";
import { isAppError } from "../../api/backend";
import type { GiftAccount } from "../../api/generated/GiftAccount";
import type { GiftBrand } from "../../api/generated/GiftBrand";
import type { GiftCardProduct } from "../../api/generated/GiftCardProduct";
import type { NetworkInfo } from "../../api/generated/NetworkInfo";
import type { WalletSummary } from "../../api/generated/WalletSummary";
import { useBackend } from "../../api/queries";
import { DashButton, Icon } from "../../components/basics";
import { CopyValue, DetailRow } from "../../components/detail";
import { LoadingState } from "../../components/history";
import { useT } from "../../i18n";
import { Page } from "../Routes";
import { FormField } from "../pay/form";
import { Checkout } from "./Checkout";
import "./giftCards.css";

const US_STATES = "AL AK AZ AR CA CO CT DE DC FL GA HI ID IL IN IA KS KY LA ME MD MA MI MN MS MO MT NE NV NH NJ NM NY NC ND OH OK OR PA RI SC SD TN TX UT VT VA WA WV WI WY".split(" ");

function problem(error: unknown, fallback: string): string {
  return isAppError(error) ? error.message : fallback;
}

function dollars(cents: number): string {
  return (cents / 100).toFixed(2);
}

function parseCents(text: string): number | null {
  return /^\d{1,4}(\.\d{1,2})?$/.test(text) ? Math.round(Number(text) * 100) : null;
}

export function GiftCardsPage({ wallet, network }: { wallet: WalletSummary; network: NetworkInfo }) {
  const t = useT();
  return (
    <Page route="explore">
      {network.kind === "mainnet" ? <PiggyCards wallet={wallet} network={network} /> : (
        <p className="placeholder card-l1">{t("giftCards.unavailable")}</p>
      )}
    </Page>
  );
}

function PiggyCards({ wallet, network }: { wallet: WalletSummary; network: NetworkInfo }) {
  const t = useT();
  const b = useBackend();
  const client = useQueryClient();
  const { walletId } = wallet;
  const accountKey = ["giftCards", walletId, "account"] as const;
  const account = useQuery({ queryKey: accountKey, queryFn: () => b.giftCardAccount(walletId), gcTime: 0, retry: false });
  const stored = useQuery({ queryKey: ["giftCards", walletId, "accounts"], queryFn: () => b.giftCardAccounts(walletId), gcTime: 0, retry: false });
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [another, setAnother] = useState(false);
  const [terms, setTerms] = useState(false);
  const working = useRef(false);

  async function run(action: () => Promise<GiftAccount>) {
    if (working.current) return;
    working.current = true;
    setBusy(true);
    setError(null);
    try {
      client.setQueryData(accountKey, await action());
      setAnother(false);
      setTerms(false);
      void stored.refetch();
    } catch (e) {
      setError(problem(e, t("giftCards.providerError")));
      void account.refetch();
    } finally {
      working.current = false;
      setBusy(false);
    }
  }

  if (account.isPending) return <LoadingState testId="gift-account-loading" text={t("giftCards.checkingAccount")} />;
  if (!account.data) return (
    <section className="card-l1 gift-panel">
      <p className="form-problem" role="alert">{problem(account.error, t("giftCards.providerError"))}</p>
      <DashButton variant="tintedBlue" disabled={account.isFetching} onClick={() => void account.refetch()}>{t("giftCards.retry")}</DashButton>
    </section>
  );

  const status = account.data;
  return (
    <>
      {error && <p className="form-problem" role="alert">{error}</p>}
      {status.kind === "signedIn" ? (
        <>
          <section className="card-l1 gift-panel gift-account">
            <div>
              <h2>{t("giftCards.provider")}</h2>
              <p className="field-hint">{t("giftCards.account", { email: status.email })}</p>
            </div>
            <DashButton variant="tintedGray" disabled={busy} onClick={() => void run(() => b.giftCardSignOut(walletId))}>{t("giftCards.signOut")}</DashButton>
          </section>
          <Catalog key={status.email} wallet={wallet} network={network} />
        </>
      ) : status.kind === "codeSent" ? (
        <form className="card-l1 gift-panel" onSubmit={(e) => {
          e.preventDefault();
          const code = new FormData(e.currentTarget).get("code");
          if (typeof code === "string") void run(() => b.giftCardVerifyEmail(walletId, code));
        }}>
          <h2>{t("giftCards.provider")}</h2>
          <p>{t("giftCards.codeSent", { email: status.email })}</p>
          <FormField label={t("giftCards.emailCode")}>
            {(p) => <input {...p} name="code" required inputMode="numeric" pattern="[0-9]{4,12}" maxLength={12} autoComplete="one-time-code" />}
          </FormField>
          <div className="gift-actions">
            <DashButton type="submit" variant="filledBlue" disabled={busy}>{t("giftCards.verify")}</DashButton>
            <DashButton variant="tintedGray" disabled={busy} onClick={() => void run(() => b.giftCardSignOut(walletId))}>{t("giftCards.changeEmail")}</DashButton>
          </div>
        </form>
      ) : status.kind === "canSignIn" && !another ? (
        <section className="card-l1 gift-panel">
          <h2>{t("giftCards.provider")}</h2>
          <p>{status.email}</p>
          <div className="gift-actions">
            <DashButton variant="filledBlue" disabled={busy} onClick={() => void run(() => b.giftCardSignIn(walletId, null))}>{t("giftCards.signIn")}</DashButton>
            <DashButton variant="tintedGray" disabled={busy} onClick={() => { setError(null); setAnother(true); }}>{t("giftCards.anotherAccount")}</DashButton>
          </div>
        </section>
      ) : (
        <form className="card-l1 gift-panel" onSubmit={(e) => {
          e.preventDefault();
          const fields = new FormData(e.currentTarget);
          const read = (name: string) => fields.get(name)?.toString() ?? "";
          void run(() => b.giftCardSignup(walletId, { firstName: read("firstName"), lastName: read("lastName"), email: read("email"), state: read("state") }, terms));
        }}>
          <h2>{t("giftCards.provider")}</h2>
          {(stored.data ?? []).filter((a) => !a.active).length > 0 && (
            <div className="gift-stored">
              <p className="field-hint">{t("giftCards.storedAccounts")}</p>
              {(stored.data ?? []).filter((a) => !a.active).map((a) => (
                <DashButton key={a.id} variant="tintedGray" disabled={busy} onClick={() => void run(() => b.giftCardSignIn(walletId, a.id))}>
                  {t("giftCards.signInAs", { email: a.email })}
                </DashButton>
              ))}
            </div>
          )}
          <p>{t("giftCards.disclosure")}</p>
          <div className="gift-fields">
            <FormField label={t("giftCards.firstName")}>
              {(p) => <input {...p} name="firstName" required maxLength={80} autoComplete="given-name" />}
            </FormField>
            <FormField label={t("giftCards.lastName")}>
              {(p) => <input {...p} name="lastName" required maxLength={80} autoComplete="family-name" />}
            </FormField>
            <FormField label={t("giftCards.email")}>
              {(p) => <input {...p} name="email" type="email" required maxLength={254} autoComplete="email" />}
            </FormField>
            <FormField label={t("giftCards.state")}>
              {(p) => <select {...p} name="state" required defaultValue="" autoComplete="address-level1">
                <option value="" disabled>{t("giftCards.chooseState")}</option>
                {US_STATES.map((state) => <option key={state}>{state}</option>)}
              </select>}
            </FormField>
          </div>
          <div className="gift-terms">
            <DashButton variant="tintedGray" disabled={busy} onClick={() => {
              void b.giftCardOpenTerms().catch((e: unknown) => setError(problem(e, t("giftCards.providerError"))));
            }}>{t("giftCards.readTerms")}</DashButton>
            <label>
              <input type="checkbox" checked={terms} onChange={(e) => setTerms(e.currentTarget.checked)} />
              {t("giftCards.acceptTerms")}
            </label>
          </div>
          <DashButton type="submit" variant="filledBlue" disabled={busy || !terms}>{t("giftCards.sendCode")}</DashButton>
        </form>
      )}
    </>
  );
}

function Catalog({ wallet, network }: { wallet: WalletSummary; network: NetworkInfo }) {
  const t = useT();
  const b = useBackend();
  const client = useQueryClient();
  const { walletId } = wallet;
  const keys = ["giftCards", walletId] as const;
  const brands = useQuery({ queryKey: [...keys, "brands"], queryFn: () => b.giftCardBrands(walletId), gcTime: 0, retry: false });
  const orders = useQuery({ queryKey: [...keys, "orders"], queryFn: () => b.giftCardOrders(walletId), gcTime: 0, retry: false });
  const [search, setSearch] = useState("");
  const [brand, setBrand] = useState<GiftBrand | null>(null);
  const [orderId, setOrderId] = useState<string | null>(null);
  const merchants = (brands.data ?? []).filter((m) => m.name.toLocaleLowerCase().includes(search.trim().toLocaleLowerCase()));

  return (
    <>
      <section className="card-l1 gift-panel">
        <h2>{t("giftCards.merchants")}</h2>
        <label className="search-field gift-search">
          <Icon id="searchMagnifyingGlass" size={16} className="search-icon" />
          <input type="search" aria-label={t("giftCards.search")} placeholder={t("giftCards.search")}
            value={search} onChange={(e) => setSearch(e.target.value)} />
        </label>
        {brands.isPending && <LoadingState testId="gift-brands-loading" text={t("giftCards.loadingMerchants")} />}
        {brands.isError && <p className="form-problem" role="alert">{problem(brands.error, t("giftCards.providerError"))}</p>}
        {brands.isSuccess && merchants.length === 0 && <p className="field-hint">{t("giftCards.noMatches")}</p>}
        {merchants.length > 0 && (
          <ul className="gift-brands">
            {merchants.map((m) => (
              <li key={m.id}>
                <DashButton variant={brand?.id === m.id ? "tintedBlue" : "tintedGray"} aria-pressed={brand?.id === m.id}
                  onClick={() => { setBrand(m); setOrderId(null); }}>{m.name}</DashButton>
              </li>
            ))}
          </ul>
        )}
      </section>
      {brand && <Products key={brand.id} brand={brand} wallet={wallet} onOrder={(id) => {
        setOrderId(id);
        setBrand(null);
        void client.invalidateQueries({ queryKey: [...keys, "orders"] });
      }} />}
      <section className="card-l1 gift-panel">
        <h2>{t("giftCards.myCards")}</h2>
        {orders.isPending && <LoadingState testId="gift-orders-loading" text={t("giftCards.loadingOrder")} />}
        {orders.isError && <p className="form-problem" role="alert">{problem(orders.error, t("giftCards.providerError"))}</p>}
        {orders.isSuccess && orders.data.length === 0 && <p className="field-hint">{t("giftCards.noOrders")}</p>}
        {(orders.data ?? []).map((order) => (
          <DashButton key={order.id} variant={orderId === order.id ? "tintedBlue" : "tintedGray"} aria-pressed={orderId === order.id}
            onClick={() => { setOrderId(order.id); setBrand(null); }}>{order.name} · ${dollars(order.cents)} USD · {order.id}</DashButton>
        ))}
      </section>
      {orderId && <OrderDetails key={orderId} walletId={walletId} orderId={orderId} network={network} />}
    </>
  );
}

function Products({ brand, wallet, onOrder }: { brand: GiftBrand; wallet: WalletSummary; onOrder: (id: string) => void }) {
  const t = useT();
  const b = useBackend();
  const products = useQuery({
    queryKey: ["giftCards", wallet.walletId, "products", brand.id],
    queryFn: () => b.giftCardProducts(wallet.walletId, brand.id), gcTime: 0, retry: false,
  });
  const [id, setId] = useState("");
  const items = products.data ?? [];
  const selected = items.find((p) => String(p.id) === id) ?? items[0];

  return (
    <section className="card-l1 gift-panel">
      <h2>{brand.name}</h2>
      {products.isPending && <LoadingState testId="gift-products-loading" text={t("giftCards.loadingProducts")} />}
      {products.isError && <p className="form-problem" role="alert">{problem(products.error, t("giftCards.providerError"))}</p>}
      {products.isSuccess && items.length === 0 && <p className="field-hint">{t("giftCards.noProducts")}</p>}
      {items.length > 1 && (
        <FormField label={t("giftCards.card")}>
          {(p) => <select {...p} value={selected?.id} onChange={(e) => setId(e.target.value)}>
            {items.map((p) => <option key={p.id} value={p.id}>{p.name}</option>)}
          </select>}
        </FormField>
      )}
      {selected && <Denomination key={selected.id} product={selected} wallet={wallet} brandId={brand.id} onOrder={onOrder} />}
    </section>
  );
}

function Denomination({ product, wallet, brandId, onOrder }: {
  product: GiftCardProduct;
  wallet: WalletSummary;
  brandId: string;
  onOrder: (id: string) => void;
}) {
  const t = useT();
  const b = useBackend();
  const [value, setValue] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const working = useRef(false);
  const cents = parseCents(value);
  const min = product.min;
  const max = product.max;
  const range = min !== null && max !== null;
  const valid = cents !== null && (range ? cents >= min && cents <= max : product.values.includes(cents));
  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (!valid || working.current || wallet.watchOnly) return;
    working.current = true;
    setBusy(true);
    setError(null);
    try {
      const order = await b.giftCardCreateOrder(wallet.walletId, brandId, product.id, cents);
      onOrder(order.id);
    } catch (err) {
      setError(problem(err, t("giftCards.providerError")));
    } finally {
      working.current = false;
      setBusy(false);
    }
  };

  return (
    <form className="gift-fields" onSubmit={(e) => void submit(e)}>
      <FormField label={t("giftCards.value")} hint={range ? t("giftCards.range", { min: dollars(min), max: dollars(max) }) : undefined}>
        {(p) => range ? (
          <input {...p} inputMode="decimal" value={value} maxLength={7} onChange={(e) => setValue(e.target.value)} />
        ) : (
          <select {...p} value={value} onChange={(e) => setValue(e.target.value)}>
            <option value="" disabled>{t("giftCards.chooseValue")}</option>
            {product.values.map((c) => <option key={c} value={dollars(c)}>{dollars(c)}</option>)}
          </select>
        )}
      </FormField>
      {wallet.watchOnly && <p className="field-hint">{t("giftCards.watchOnly")}</p>}
      {error && <p className="form-problem" role="alert">{error}</p>}
      <DashButton type="submit" variant="filledBlue" disabled={busy || !valid || wallet.watchOnly}>{t("giftCards.quote")}</DashButton>
    </form>
  );
}

function OrderDetails({ walletId, orderId, network }: { walletId: string; orderId: string; network: NetworkInfo }) {
  const t = useT();
  const b = useBackend();
  const info = useQuery({
    queryKey: ["giftCards", walletId, "detail", orderId],
    queryFn: () => b.giftCardDetails(walletId, orderId),
    gcTime: 0, retry: false, refetchOnWindowFocus: false,
    refetchInterval: ({ state }) => {
      const data = state.data;
      if (!data || state.status === "error") return false;
      return data.status === "processing" || (data.status === "awaitingPayment" &&
        (data.order.txid !== null || (data.order.invoice === null && data.order.expiresAt * 1000 > Date.now()))) ? 5000 : false;
    },
  });
  const details = info.data;
  const [claimError, setClaimError] = useState<string | null>(null);
  const openClaim = (card: number, host: string) => {
    setClaimError(null);
    void b.giftCardOpenClaim(walletId, orderId, card, host)
      .catch((e: unknown) => setClaimError(problem(e, t("giftCards.providerError"))));
  };

  return (
    <section className="card-l1 gift-panel" aria-label={t("giftCards.order")}>
      {info.isPending && <LoadingState testId="gift-order-loading" text={t("giftCards.loadingOrder")} />}
      {info.isError && <p className="form-problem" role="alert">{problem(info.error, t("giftCards.providerError"))}</p>}
      {details && (
        <>
          <h2>{details.order.name} · ${dollars(details.order.cents)} USD</h2>
          <p className="gift-status" role="status">{t(`giftCards.status.${details.status}`)}</p>
          {details.order.invoice && <Checkout details={details} invoice={details.order.invoice} network={network} walletId={walletId} />}
          {details.status === "awaitingPayment" && details.order.invoice === null && (
            <p className="field-hint">{t(Date.now() >= details.order.expiresAt * 1000 ? "giftCards.expired" : "giftCards.invoicePending")}</p>
          )}
          {details.status === "complete" && (
            details.cards.some((c) => c.code || c.pin || c.claimHost) ? details.cards.map((card, i) => (
              <div className="gift-redemption" key={i}>
                {(card.code || card.pin) && (
                  <dl className="detail-list">
                    {card.code && <DetailRow label={t("giftCards.claimCode")}><CopyValue value={card.code} label={t("giftCards.claimCode")} /></DetailRow>}
                    {card.pin && <DetailRow label={t("giftCards.pin")}><CopyValue value={card.pin} label={t("giftCards.pin")} /></DetailRow>}
                  </dl>
                )}
                {card.claimHost && <DashButton variant="tintedBlue" onClick={() => openClaim(i, card.claimHost!)}>
                  {t("giftCards.redeem", { host: card.claimHost })}
                </DashButton>}
              </div>
            )) : <p className="field-hint">{t("giftCards.noCodes")}</p>
          )}
          {claimError && <p className="form-problem" role="alert">{claimError}</p>}
        </>
      )}
      <DashButton variant="tintedGray" disabled={info.isFetching} onClick={() => void info.refetch()}>{t("giftCards.refresh")}</DashButton>
    </section>
  );
}
