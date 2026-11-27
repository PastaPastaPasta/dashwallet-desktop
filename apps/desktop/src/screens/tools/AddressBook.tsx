// Tools ▸ Address Book (QT-095…098): sending and receiving entries with dash-qt's wildcard
// search (D13), New for sending addresses, Edit (the label, D14), Delete for sending entries,
// copy, a QR, and the CSV export through the save dialog (D2). The picker (QT-097) is the same
// list in a sheet: Pay offers the sending book, Sign the receiving book.

import { useEffect, useId, useRef, useState, type ReactNode } from "react";
import { useQuery } from "@tanstack/react-query";
import type { AddressBookEntry } from "../../api/generated/AddressBookEntry";
import type { AddressPurposeName } from "../../api/generated/AddressPurposeName";
import type { NetworkInfo } from "../../api/generated/NetworkInfo";
import { useBackend, useQrMatrix } from "../../api/queries";
import { fileErrorText, saveText } from "../../app/files";
import { navigate } from "../../app/ui";
import { DashButton } from "../../components/basics";
import { CopyValue } from "../../components/detail";
import { addressBookEmpty, type AddressBookStep } from "../../components/emptyStates";
import { EmptyState, focusWhenIdle, LoadingState } from "../../components/history";
import { QrCode, QrPlaceholder } from "../../components/qr";
import { Sheet } from "../../components/sheet";
import { showToast } from "../../components/toast";
import { useT } from "../../i18n";
import { useModel } from "../../onboarding/model";
import { AddressBookModel, AddressBookText } from "../../tools/addressBook";
import { Outcome, RowMenu } from "./common";

const PURPOSES: { id: AddressPurposeName; key: "sending" | "receiving" }[] = [
  { id: "send", key: "sending" },
  { id: "receive", key: "receiving" },
];

function AddressBookEmpty({ model, actions }: { model: AddressBookModel; actions: Partial<Record<AddressBookStep, () => void>> }) {
  const copy = addressBookEmpty({ search: model.search, purpose: model.purpose, count: 0 });
  if (!copy) return null;
  const go = actions[copy.step];
  return (
    <EmptyState
      testId="address-book-none"
      art={copy.art}
      title={copy.title}
      message={copy.message}
      action={
        copy.action && go ? (
          <DashButton variant="filledBlue" data-testid={copy.action.testId} onClick={go}>
            {copy.action.label}
          </DashButton>
        ) : undefined
      }
    />
  );
}

/** The search field and the purpose's entries, each with `actions`. */
function AddressList({
  model,
  actions,
  onOpen,
  emptyActions,
}: {
  model: AddressBookModel;
  actions: (e: AddressBookEntry) => ReactNode;
  onOpen?: (e: AddressBookEntry) => void;
  /** Steps this list may take. The picker omits any that would leave the sheet. */
  emptyActions: Partial<Record<AddressBookStep, () => void>>;
}) {
  const t = useT();
  const id = useId();
  const searchRef = useRef<HTMLInputElement>(null);
  const entries = model.entries;
  const steps: Partial<Record<AddressBookStep, () => void>> = {
    ...emptyActions,
    clear: () => {
      model.setSearch("");
      focusWhenIdle(searchRef);
    },
  };
  return (
    <>
      <label className="sr-only" htmlFor={`${id}-search`}>
        {t("tools.addressBook.search")}
      </label>
      <input
        ref={searchRef}
        id={`${id}-search`}
        className="tool-field"
        type="search"
        data-testid="address-book-search"
        placeholder={AddressBookText.searchPlaceholder}
        value={model.search}
        onChange={(e) => model.setSearch(e.target.value)}
      />
      {model.all === null ? (
        !model.error && <LoadingState testId="address-book-loading" text={t("tools.addressBook.loading")} />
      ) : entries.length === 0 ? (
        <AddressBookEmpty model={model} actions={steps} />
      ) : (
        <div className="table-card">
          <table className="data-table address-book-table" data-testid="address-book-table">
            <caption className="sr-only">{model.purpose === "send" ? t("tools.addressBook.sending") : t("tools.addressBook.receiving")}</caption>
            <thead>
              <tr>
                <th scope="col">{t("tools.addressBook.columns.label")}</th>
                <th scope="col">{t("tools.addressBook.columns.address")}</th>
                <th scope="col">
                  <span className="sr-only">{t("tools.addressBook.columns.actions")}</span>
                </th>
              </tr>
            </thead>
            <tbody>
              {entries.map((e) => (
                <tr key={e.address} data-testid={`address-book-entry-${e.address}`} onDoubleClick={onOpen && (() => onOpen(e))}>
                  <td>{e.label ? <bdi>{e.label}</bdi> : model.labelText(e)}</td>
                  <td className="mono address-cell">
                    <bdi>{e.address}</bdi>
                  </td>
                  <td className="row-actions">{actions(e)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </>
  );
}

function PurposeSwitch({ model }: { model: AddressBookModel }) {
  const t = useT();
  return (
    <div className="segmented" role="group" aria-label={t("tools.addressBook.listLabel")} data-testid="address-book-purpose">
      {PURPOSES.map((p) => (
        <button key={p.id} type="button" className="segment" aria-pressed={model.purpose === p.id} data-testid={`address-book-${p.id}`} onClick={() => model.setPurpose(p.id)}>
          {t(`tools.addressBook.${p.key}`)}
        </button>
      ))}
    </div>
  );
}

/** New (an address and a label) or Edit (the label; the address read-only, D14). */
function EntrySheet({ model, entry, onClose }: { model: AddressBookModel; entry: AddressBookEntry | null; onClose: () => void }) {
  const t = useT();
  const [address, setAddress] = useState(entry?.address ?? "");
  const [label, setLabel] = useState(entry?.label ?? "");
  const [busy, setBusy] = useState(false);
  const id = useId();
  const title = entry ? (entry.purpose === "send" ? AddressBookText.editSending : AddressBookText.editReceiving) : AddressBookText.newSending;
  return (
    <Sheet title={title} width="small" testId="address-book-editor" onClose={onClose}>
      <form
        className="tool-stack"
        onSubmit={(e) => {
          e.preventDefault();
          setBusy(true);
          void model.save(address, label, entry !== null).then((saved) => (saved ? onClose() : setBusy(false)));
        }}
      >
        <div className="form-field">
          <label htmlFor={`${id}-label`}>{t("tools.addressBook.columns.label")}</label>
          <input id={`${id}-label`} data-testid="address-book-label" value={label} autoComplete="off" autoFocus onChange={(e) => setLabel(e.target.value)} />
        </div>
        <div className="form-field">
          <label htmlFor={`${id}-address`}>{t("tools.addressBook.columns.address")}</label>
          <input
            id={`${id}-address`}
            className="mono"
            data-testid="address-book-address"
            value={address}
            readOnly={entry !== null}
            autoComplete="off"
            spellCheck={false}
            aria-invalid={!!model.error}
            aria-describedby={model.error ? `${id}-error` : undefined}
            onChange={(e) => setAddress(e.target.value)}
          />
        </div>
        {model.error && (
          <p id={`${id}-error`} className="form-problem" role="alert" data-testid="address-book-error">
            {model.error}
          </p>
        )}
        <div className="sheet-actions">
          <DashButton variant="tintedGray" size="large" data-testid="address-book-cancel" onClick={onClose}>
            {t("common.cancel")}
          </DashButton>
          <DashButton type="submit" variant="filledBlue" size="large" data-testid="address-book-save" disabled={busy || address.trim() === ""}>
            {t("tools.addressBook.ok")}
          </DashButton>
        </div>
      </form>
    </Sheet>
  );
}

function QrSheet({ entry, onClose }: { entry: AddressBookEntry; onClose: () => void }) {
  const t = useT();
  const b = useBackend();
  const uri = useQuery({
    queryKey: ["paymentUri", entry.address, entry.label] as const,
    queryFn: () => b.buildPaymentUri(entry.address, null, entry.label || null, null, null),
  });
  const qr = useQrMatrix(uri.data);
  return (
    <Sheet title={entry.label || AddressBookText.noLabel} width="small" testId="address-book-qr" onClose={onClose}>
      <div className="tool-qr">
        {qr.data ? <QrCode matrix={qr.data} size={240} label={t("tools.addressBook.qrLabel", { address: entry.address })} testId="address-book-qr-code" /> : <QrPlaceholder size={240} testId="address-book-qr-loading" label={t("tools.addressBook.qrLoading")} />}
        <div className="tool-qr-value">
          <CopyValue value={uri.data ?? entry.address} action={t("tools.addressBook.copyPaymentLink")} mono />
        </div>
      </div>
    </Sheet>
  );
}

export function AddressBookTab({ walletId, network }: { walletId: string; network: NetworkInfo }) {
  const t = useT();
  const b = useBackend();
  const [model] = useState(() => new AddressBookModel(b, walletId, network.dirName));
  useModel(model);
  // `undefined`: closed; `null`: New.
  const [editing, setEditing] = useState<AddressBookEntry | null | undefined>(undefined);
  useEffect(() => void model.load(), [model]);

  const exportCsv = async () => {
    try {
      const path = await saveText(b, "csv", model.exportFileName, model.exportCsv());
      if (path !== null) showToast(t("tools.addressBook.exported", { path }));
    } catch (e) {
      showToast(t("tools.addressBook.exportFailed", { error: fileErrorText(e) }));
    }
  };

  const edit = (entry: AddressBookEntry | null | undefined) => {
    model.clearError();
    setEditing(entry);
  };

  return (
    <div className="tool-stack" data-testid="tools-addressBook">
      <div className="tool-actions">
        <PurposeSwitch model={model} />
        {model.canCreate && (model.all === null || model.search !== "" || model.all.some((e) => e.purpose === model.purpose)) && (
          <DashButton variant="tintedBlue" size="small" data-testid="address-book-new" onClick={() => edit(null)}>
            {t("tools.addressBook.new")}
          </DashButton>
        )}
        <DashButton variant="tintedGray" size="small" data-testid="address-book-export" disabled={model.entries.length === 0} onClick={() => void exportCsv()}>
          {t("tools.addressBook.export")}
        </DashButton>
      </div>
      <p className="placeholder-note" data-testid="address-book-header">
        {model.header}
      </p>
      <Outcome ok={false} text={editing === undefined ? model.error : null} testId="address-book-problem" />
      <AddressList
        model={model}
        emptyActions={{
          ...(model.canCreate ? { add: () => edit(null) } : {}),
          receive: () => navigate("receive"),
        }}
        actions={(e) => (
          <span className="tool-row-actions">
            <RowMenu
              label={t("tools.addressBook.copy")}
              name={t("tools.addressBook.copyEntry", { label: model.labelText(e) })}
              items={[
                { id: "copy-address", title: t("tools.addressBook.copyAddress"), copy: e.address },
                { id: "copy-label", title: t("tools.addressBook.copyLabel"), copy: e.label },
              ]}
              testId={`address-book-copy-${e.address}`}
            />
            <DashButton variant="plainBlue" size="small" data-testid={`address-book-qr-${e.address}`} aria-label={t("tools.addressBook.showQr", { label: model.labelText(e) })} onClick={() => model.showQr(e)}>
              {t("tools.addressBook.qr")}
            </DashButton>
            <DashButton variant="plainBlue" size="small" data-testid={`address-book-edit-${e.address}`} aria-label={t("tools.addressBook.editEntry", { label: model.labelText(e) })} onClick={() => edit(e)}>
              {t("tools.addressBook.edit")}
            </DashButton>
            {model.canDelete && (
              <DashButton variant="plainRed" size="small" data-testid={`address-book-delete-${e.address}`} aria-label={t("tools.addressBook.deleteEntry", { label: model.labelText(e) })} onClick={() => void model.delete(e.address)}>
                {t("tools.addressBook.delete")}
              </DashButton>
            )}
          </span>
        )}
      />
      {editing !== undefined && <EntrySheet model={model} entry={editing} onClose={() => edit(undefined)} />}
      {model.qrEntry && <QrSheet entry={model.qrEntry} onClose={() => model.showQr(null)} />}
    </div>
  );
}

/** QT-097: an address book list to choose from; Choose or a double click hands the address back. */
export function AddressBookPicker({
  walletId,
  network,
  purpose,
  onChoose,
  onClose,
}: {
  walletId: string;
  network: NetworkInfo;
  purpose: AddressPurposeName;
  onChoose: (address: string, label: string) => void;
  onClose: () => void;
}) {
  const t = useT();
  const b = useBackend();
  const [model] = useState(() => new AddressBookModel(b, walletId, network.dirName, purpose, true));
  useModel(model);
  useEffect(() => void model.load(), [model]);
  useEffect(() => {
    if (model.chosen) onChoose(model.chosen.address, model.chosen.label);
  }, [model.chosen, onChoose]);
  return (
    <Sheet title={purpose === "send" ? t("tools.addressBook.chooseSend") : t("tools.addressBook.chooseSign")} width="medium" testId="address-book-picker" onClose={onClose}>
      <div className="tool-stack">
        <p className="placeholder-note">{model.header}</p>
        <Outcome ok={false} text={model.error} testId="address-book-problem" />
        <AddressList
          model={model}
          emptyActions={{}}
          onOpen={(e) => model.choose(e)}
          actions={(e) => (
            <DashButton variant="tintedBlue" size="small" data-testid={`address-book-choose-${e.address}`} aria-label={t("tools.addressBook.chooseEntry", { label: model.labelText(e) })} onClick={() => model.choose(e)}>
              {t("tools.addressBook.choose")}
            </DashButton>
          )}
        />
      </div>
    </Sheet>
  );
}
