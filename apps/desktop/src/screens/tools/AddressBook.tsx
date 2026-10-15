// Tools ▸ Address Book (QT-095…098): sending and receiving entries with dash-qt's wildcard
// search (D13), New for sending addresses, Edit (the label, D14), Delete for sending entries,
// copy, a QR, and the CSV export through the save dialog (D2). The picker (QT-097) is the same
// list in a sheet: Pay offers the sending book, Sign the receiving book.

import { useEffect, useId, useState, type ReactNode } from "react";
import { useQuery } from "@tanstack/react-query";
import type { AddressBookEntry } from "../../api/generated/AddressBookEntry";
import type { AddressPurposeName } from "../../api/generated/AddressPurposeName";
import type { NetworkInfo } from "../../api/generated/NetworkInfo";
import { useBackend, useQrMatrix } from "../../api/queries";
import { fileErrorText, saveText } from "../../app/files";
import { DashButton } from "../../components/basics";
import { CopyValue } from "../../components/detail";
import { LoadingState } from "../../components/history";
import { QrCode } from "../../components/qr";
import { Sheet } from "../../components/sheet";
import { useModel } from "../../onboarding/model";
import { AddressBookModel, AddressBookText } from "../../tools/addressBook";
import { Outcome, RowMenu } from "./common";

const PURPOSES: { id: AddressPurposeName; title: string }[] = [
  { id: "send", title: "Sending addresses" },
  { id: "receive", title: "Receiving addresses" },
];

/** The search field and the purpose's entries, each with `actions`. */
function AddressList({ model, actions, onOpen }: { model: AddressBookModel; actions: (e: AddressBookEntry) => ReactNode; onOpen?: (e: AddressBookEntry) => void }) {
  const id = useId();
  const entries = model.entries;
  return (
    <>
      <label className="sr-only" htmlFor={`${id}-search`}>
        Search
      </label>
      <input
        id={`${id}-search`}
        className="tool-field"
        type="search"
        data-testid="address-book-search"
        placeholder={AddressBookText.searchPlaceholder}
        value={model.search}
        onChange={(e) => model.setSearch(e.target.value)}
      />
      {model.all === null ? (
        !model.error && <LoadingState testId="address-book-loading" text="Reading the address book" />
      ) : entries.length === 0 ? (
        <p className="placeholder-note" data-testid="address-book-none">
          {model.search ? "No entry matches the search." : "No entries yet."}
        </p>
      ) : (
        <div className="table-card">
          <table className="data-table address-book-table" data-testid="address-book-table">
            <caption className="sr-only">{model.purpose === "send" ? "Sending addresses" : "Receiving addresses"}</caption>
            <thead>
              <tr>
                <th scope="col">Label</th>
                <th scope="col">Address</th>
                <th scope="col">
                  <span className="sr-only">Actions</span>
                </th>
              </tr>
            </thead>
            <tbody>
              {entries.map((e) => (
                <tr key={e.address} data-testid={`address-book-entry-${e.address}`} onDoubleClick={onOpen && (() => onOpen(e))}>
                  <td>{model.labelText(e)}</td>
                  <td className="mono address-cell">{e.address}</td>
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
  return (
    <div className="segmented" role="group" aria-label="Address list" data-testid="address-book-purpose">
      {PURPOSES.map((p) => (
        <button key={p.id} type="button" className="segment" aria-pressed={model.purpose === p.id} data-testid={`address-book-${p.id}`} onClick={() => model.setPurpose(p.id)}>
          {p.title}
        </button>
      ))}
    </div>
  );
}

/** New (an address and a label) or Edit (the label; the address read-only, D14). */
function EntrySheet({ model, entry, onClose }: { model: AddressBookModel; entry: AddressBookEntry | null; onClose: () => void }) {
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
          <label htmlFor={`${id}-label`}>Label</label>
          <input id={`${id}-label`} data-testid="address-book-label" value={label} autoComplete="off" autoFocus onChange={(e) => setLabel(e.target.value)} />
        </div>
        <div className="form-field">
          <label htmlFor={`${id}-address`}>Address</label>
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
            Cancel
          </DashButton>
          <DashButton type="submit" variant="filledBlue" size="large" data-testid="address-book-save" disabled={busy || address.trim() === ""}>
            OK
          </DashButton>
        </div>
      </form>
    </Sheet>
  );
}

function QrSheet({ entry, onClose }: { entry: AddressBookEntry; onClose: () => void }) {
  const b = useBackend();
  const uri = useQuery({
    queryKey: ["paymentUri", entry.address, entry.label] as const,
    queryFn: () => b.buildPaymentUri(entry.address, null, entry.label || null, null, null),
  });
  const qr = useQrMatrix(uri.data);
  return (
    <Sheet title={entry.label || AddressBookText.noLabel} width="small" testId="address-book-qr" onClose={onClose}>
      <div className="tool-qr">
        {qr.data ? <QrCode matrix={qr.data} size={240} label={`QR code of ${entry.address}`} testId="address-book-qr-code" /> : <LoadingState testId="address-book-qr-loading" text="Drawing the QR code" />}
        <CopyValue value={uri.data ?? entry.address} label="payment link" mono />
      </div>
    </Sheet>
  );
}

export function AddressBookTab({ walletId, network }: { walletId: string; network: NetworkInfo }) {
  const b = useBackend();
  const [model] = useState(() => new AddressBookModel(b, walletId, network.dirName));
  useModel(model);
  // `undefined`: closed; `null`: New.
  const [editing, setEditing] = useState<AddressBookEntry | null | undefined>(undefined);
  const [saved, setSaved] = useState<{ ok: boolean; text: string } | null>(null);
  useEffect(() => void model.load(), [model]);

  const exportCsv = async () => {
    setSaved(null);
    try {
      const path = await saveText(b, "csv", model.purpose === "send" ? "Sending addresses.csv" : "Receiving addresses.csv", model.exportCsv());
      if (path !== null) setSaved({ ok: true, text: `Exported to ${path}.` });
    } catch (e) {
      setSaved({ ok: false, text: `There was an error trying to save the address list. ${fileErrorText(e)}` });
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
        {model.canCreate && (
          <DashButton variant="tintedBlue" size="small" data-testid="address-book-new" onClick={() => edit(null)}>
            New
          </DashButton>
        )}
        <DashButton variant="tintedGray" size="small" data-testid="address-book-export" disabled={model.entries.length === 0} onClick={() => void exportCsv()}>
          Export…
        </DashButton>
      </div>
      <p className="placeholder-note" data-testid="address-book-header">
        {model.header}
      </p>
      {saved && <Outcome ok={saved.ok} text={saved.text} testId="address-book-exported" />}
      {model.error && editing === undefined && <Outcome ok={false} text={model.error} testId="address-book-problem" />}
      <AddressList
        model={model}
        actions={(e) => (
          <span className="tool-row-actions">
            <RowMenu
              label="Copy"
              items={[
                { title: "Copy address", copy: e.address },
                { title: "Copy label", copy: e.label },
              ]}
              testId={`address-book-copy-${e.address}`}
            />
            <DashButton variant="plainBlue" size="small" data-testid={`address-book-qr-${e.address}`} aria-label={`Show the QR code of ${model.labelText(e)}`} onClick={() => model.showQr(e)}>
              QR
            </DashButton>
            <DashButton variant="plainBlue" size="small" data-testid={`address-book-edit-${e.address}`} aria-label={`Edit ${model.labelText(e)}`} onClick={() => edit(e)}>
              Edit
            </DashButton>
            {model.canDelete && (
              <DashButton variant="plainRed" size="small" data-testid={`address-book-delete-${e.address}`} aria-label={`Delete ${model.labelText(e)}`} onClick={() => void model.delete(e.address)}>
                Delete
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
  const b = useBackend();
  const [model] = useState(() => new AddressBookModel(b, walletId, network.dirName, purpose, true));
  useModel(model);
  useEffect(() => void model.load(), [model]);
  useEffect(() => {
    if (model.chosen) onChoose(model.chosen.address, model.chosen.label);
  }, [model.chosen, onChoose]);
  return (
    <Sheet title={purpose === "send" ? "Choose the address to send coins to" : "Choose the address to sign with"} width="medium" testId="address-book-picker" onClose={onClose}>
      <div className="tool-stack">
        <p className="placeholder-note">{model.header}</p>
        {model.error && <Outcome ok={false} text={model.error} testId="address-book-problem" />}
        <AddressList
          model={model}
          onOpen={(e) => model.choose(e)}
          actions={(e) => (
            <DashButton variant="tintedBlue" size="small" data-testid={`address-book-choose-${e.address}`} aria-label={`Choose ${model.labelText(e)}`} onClick={() => model.choose(e)}>
              Choose
            </DashButton>
          )}
        />
      </div>
    </Sheet>
  );
}
