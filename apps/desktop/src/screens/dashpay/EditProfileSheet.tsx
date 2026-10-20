// Edit Profile (DP4-03 E1–E10, U1–U3, G1–G2, K1–K3, Q1): the sheet over My Profile. The form,
// its picture's sources, the crop step and the discard question; the model is
// dashpay/editProfile.ts, one per wallet (D1).

import { useId, useRef, type ClipboardEvent, type DragEvent, type KeyboardEvent, type PointerEvent } from "react";
import { navigate } from "../../app/ui";
import { DashButton } from "../../components/basics";
import { Sheet } from "../../components/sheet";
import { MAX_ZOOM, MIN_ZOOM, PAN_STEP, ZOOM_STEP, cropRect, panBy, placement, zoomTo } from "../../dashpay/crop";
import { charCount, EDIT_COPY, type EditProfileModel } from "../../dashpay/editProfile";
import { useModel } from "../../onboarding/model";
import { Avatar } from "../contacts/Avatar";
import { GrantSheet } from "../tools/common";

const C = EDIT_COPY;
/** The crop frame's side in CSS pixels (D4). */
const FRAME = 240;

/** The file a drop or a paste carries. */
function carriedFile(data: DataTransfer | null): File | null {
  return [...(data?.files ?? [])][0] ?? null;
}

function Counted({
  label,
  value,
  max,
  over,
  placeholder,
  multiline,
  disabled,
  testId,
  onChange,
}: {
  label: string;
  value: string;
  max: number;
  over: boolean;
  placeholder?: string;
  multiline?: boolean;
  disabled: boolean;
  testId: string;
  onChange: (text: string) => void;
}) {
  const id = useId();
  const props = {
    value,
    placeholder,
    disabled,
    "aria-invalid": over || undefined,
    "aria-describedby": `${id}-count`,
    "data-testid": testId,
    onChange: (e: { target: { value: string } }) => onChange(e.target.value),
  };
  return (
    <label className="form-field edit-profile-field">
      {label}
      {multiline ? <textarea rows={3} {...props} /> : <input type="text" autoComplete="off" {...props} />}
      <span className={over ? "field-hint edit-profile-count is-over" : "field-hint edit-profile-count"} id={`${id}-count`} data-testid={`${testId}-count`}>
        {C.count(charCount(value), max)}
        {over && `. ${C.tooLong(max)}`}
      </span>
    </label>
  );
}

/** The picture block of the form (E1, E2, E4, E9, E10). */
function Picture({ m, letters, identity }: { m: EditProfileModel; letters: string; identity: string }) {
  const p = m.picture;
  const busy = m.saving !== null;
  const shown = p.kind === "new" ? <Avatar letters={letters} size="pane" png={p.candidate.preview.png} /> : p.kind === "remove" ? <Avatar letters={letters} size="pane" /> : <Avatar letters={letters} size="pane" identity={identity} own />;
  const take = (file: File | null) => {
    if (file) void m.takeFile(file);
  };
  return (
    <section
      className="edit-profile-picture"
      data-testid="edit-profile-picture"
      data-choice={p.kind}
      aria-label={C.picture.change}
      onDragOver={(e: DragEvent) => {
        if (!m.canUse("file")) return;
        e.preventDefault();
        e.dataTransfer.dropEffect = "copy";
      }}
      onDrop={(e: DragEvent) => {
        if (!m.canUse("file")) return;
        e.preventDefault();
        take(carriedFile(e.dataTransfer));
      }}
      onPaste={(e: ClipboardEvent) => {
        const file = carriedFile(e.clipboardData);
        if (!file || !m.canUse("file")) return;
        e.preventDefault();
        take(file);
      }}
    >
      {shown}
      <div className="edit-profile-picture-side">
        {p.kind !== "keep" && (
          <p className="edit-profile-picture-state" role="status" data-testid="edit-profile-picture-state">
            {p.kind === "new" ? C.picture.isNew : C.picture.isRemoved}{" "}
            <DashButton variant="plainBlue" size="small" data-testid="edit-profile-undo" disabled={busy} onClick={() => m.undoPicture()}>
              {C.picture.undo}
            </DashButton>
          </p>
        )}
        {m.picturesOn ? (
          <div className="edit-profile-sources">
            <DashButton variant="tintedBlue" size="small" data-testid="edit-profile-file" disabled={!m.canUse("file")} onClick={() => void m.chooseFile()}>
              {C.picture.chooseFile}
            </DashButton>
            <DashButton variant="tintedBlue" size="small" data-testid="edit-profile-url" disabled={!m.canUse("url")} onClick={() => m.openStep("url")}>
              {C.picture.url}
            </DashButton>
            <DashButton variant="tintedBlue" size="small" data-testid="edit-profile-gravatar" disabled={!m.canUse("gravatar")} onClick={() => m.openStep("gravatar")}>
              {C.picture.gravatar}
            </DashButton>
          </div>
        ) : (
          <p className="field-hint" data-testid="edit-profile-pictures-off">
            {C.picture.off}{" "}
            <DashButton variant="plainBlue" size="small" data-testid="edit-profile-open-settings" onClick={() => navigate("settings", { settingsTab: "general" })}>
              {C.picture.openSettings}
            </DashButton>
          </p>
        )}
        {m.picturesOn && (
          <p className="field-hint" data-testid="edit-profile-file-hint">
            {m.uploadAvailable ? C.picture.drop : C.picture.noUpload}
          </p>
        )}
        {m.hasPicture && p.kind === "keep" && (
          <DashButton variant="plainRed" size="small" data-testid="edit-profile-remove" disabled={busy} onClick={() => m.removePicture()}>
            {C.picture.remove}
          </DashButton>
        )}
        {m.pictureError && (
          <p className="form-problem" role="alert" data-testid="edit-profile-picture-error">
            {m.pictureError}
          </p>
        )}
      </div>
    </section>
  );
}

function Form({ m, letters, username }: { m: EditProfileModel; letters: string; username: string | null }) {
  const busy = m.saving !== null;
  return (
    <>
      <Picture m={m} letters={letters} identity={m.identity!} />
      <Counted
        label={C.displayName}
        value={m.displayName}
        max={m.limits.displayNameMax}
        over={m.nameOver}
        placeholder={username ?? undefined}
        disabled={busy}
        testId="edit-profile-name"
        onChange={(t) => m.setDisplayName(t)}
      />
      <Counted
        label={C.aboutMe}
        value={m.publicMessage}
        max={m.limits.publicMessageMax}
        over={m.messageOver}
        multiline
        disabled={busy}
        testId="edit-profile-message"
        onChange={(t) => m.setPublicMessage(t)}
      />
      {m.dirty && m.empty && (
        <p className="field-hint" data-testid="edit-profile-empty">
          {C.empty}
        </p>
      )}
      {m.saving && (
        <p className="edit-profile-saving" role="status" data-testid="edit-profile-saving">
          {m.saving === "upload" ? C.uploading : C.saving}
          <span className="spinner" aria-hidden="true" />
        </p>
      )}
      {m.error && !m.saving && (
        <p className="form-problem" role="alert" data-testid="edit-profile-error">
          {m.error}
        </p>
      )}
      <div className="sheet-actions">
        <DashButton variant="tintedGray" size="large" data-testid="edit-profile-cancel" disabled={busy} onClick={() => m.requestClose()}>
          {C.cancel}
        </DashButton>
        <DashButton variant="filledBlue" size="large" data-testid="edit-profile-save" disabled={!m.canSave} onClick={() => void m.save()}>
          {busy ? C.saving : C.save}
        </DashButton>
      </div>
    </>
  );
}

/** U1–U3 and G1–G2: one field, its fetch and its refusal. */
function SourceStep({ m, kind }: { m: EditProfileModel; kind: "url" | "gravatar" }) {
  const copy = kind === "url" ? C.url : C.gravatar;
  const value = kind === "url" ? m.url : m.email;
  const fetch = () => void (kind === "url" ? m.fetchUrl() : m.fetchGravatar());
  return (
    <>
      <p className="edit-profile-step-title">{copy.title}</p>
      {kind === "url" && <p className="sheet-text">{C.url.text}</p>}
      <label className="form-field">
        {copy.field}
        <input
          type={kind === "url" ? "url" : "email"}
          data-autofocus
          autoComplete="off"
          spellCheck={false}
          value={value}
          placeholder={kind === "gravatar" ? C.gravatar.placeholder : "https://"}
          disabled={m.preparing}
          aria-invalid={m.pictureError ? true : undefined}
          data-testid={`edit-profile-${kind}-field`}
          onChange={(e) => (kind === "url" ? m.setUrl(e.target.value) : m.setEmail(e.target.value))}
          onKeyDown={(e) => {
            if (e.key !== "Enter") return;
            e.preventDefault();
            fetch();
          }}
        />
      </label>
      {kind === "gravatar" && <p className="field-hint">{C.gravatar.disclaimer}</p>}
      {m.preparing && (
        <p className="edit-profile-saving" role="status" data-testid="edit-profile-fetching">
          {copy.fetching}
          <span className="spinner" aria-hidden="true" />
        </p>
      )}
      {m.pictureError && (
        <p className="form-problem" role="alert" data-testid="edit-profile-step-error">
          {m.pictureError}
        </p>
      )}
      <div className="sheet-actions">
        <DashButton variant="tintedGray" size="large" data-testid="edit-profile-back" onClick={() => m.back()}>
          {C.back}
        </DashButton>
        <DashButton variant="filledBlue" size="large" data-testid={`edit-profile-${kind}-fetch`} disabled={m.preparing || !value.trim()} onClick={fetch}>
          {copy.fetch}
        </DashButton>
      </div>
    </>
  );
}

/** K1–K2: the square frame over the picture; drag or the arrow keys move it, the slider and
 * + / − zoom (D4). */
function CropStep({ m }: { m: EditProfileModel }) {
  const drag = useRef<{ x: number; y: number } | null>(null);
  const view = m.crop;
  const rect = view ? cropRect(view) : null;
  const onPointerDown = (e: PointerEvent<HTMLDivElement>) => {
    if (!view || m.preparing) return;
    e.currentTarget.setPointerCapture(e.pointerId);
    drag.current = { x: e.clientX, y: e.clientY };
  };
  const onPointerMove = (e: PointerEvent<HTMLDivElement>) => {
    const from = drag.current;
    if (!from || !view) return;
    drag.current = { x: e.clientX, y: e.clientY };
    m.setCrop(panBy(view, e.clientX - from.x, e.clientY - from.y, FRAME));
  };
  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (!view) return;
    const step = FRAME * PAN_STEP;
    const moves: Record<string, [number, number]> = { ArrowLeft: [step, 0], ArrowRight: [-step, 0], ArrowUp: [0, step], ArrowDown: [0, -step] };
    const move = moves[e.key];
    if (move) m.setCrop(panBy(view, move[0], move[1], FRAME));
    else if (e.key === "+" || e.key === "=") m.setCrop(zoomTo(view, view.zoom + ZOOM_STEP));
    else if (e.key === "-") m.setCrop(zoomTo(view, view.zoom - ZOOM_STEP));
    else return;
    e.preventDefault();
  };
  const at = view ? placement(view, FRAME) : null;
  return (
    <>
      <p className="edit-profile-step-title">{C.crop.title}</p>
      <div
        className="crop-frame"
        tabIndex={0}
        role="img"
        aria-label={C.crop.frame}
        data-autofocus
        data-testid="edit-profile-crop-frame"
        data-crop={rect ? `${rect.x},${rect.y},${rect.width}` : undefined}
        style={{ width: FRAME, height: FRAME }}
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={() => (drag.current = null)}
        onPointerCancel={() => (drag.current = null)}
        onKeyDown={onKeyDown}
      >
        {m.file && (
          <img
            className="crop-image"
            src={m.file.src}
            alt=""
            draggable={false}
            style={at ?? { visibility: "hidden" }}
            onLoad={(e) => m.pictureLoaded(e.currentTarget.naturalWidth, e.currentTarget.naturalHeight)}
            onError={() => m.pictureFailed()}
          />
        )}
        <span className="crop-ring" aria-hidden="true" />
      </div>
      <label className="form-field crop-zoom">
        {C.crop.zoom}
        <input
          type="range"
          min={MIN_ZOOM}
          max={MAX_ZOOM}
          step={0.01}
          value={view?.zoom ?? MIN_ZOOM}
          disabled={!view || m.preparing}
          data-testid="edit-profile-crop-zoom"
          onChange={(e) => view && m.setCrop(zoomTo(view, Number(e.target.value)))}
        />
      </label>
      {m.preparing && (
        <p className="edit-profile-saving" role="status" data-testid="edit-profile-fetching">
          {C.crop.preparing}
          <span className="spinner" aria-hidden="true" />
        </p>
      )}
      <div className="sheet-actions">
        <DashButton variant="tintedGray" size="large" data-testid="edit-profile-back" disabled={m.preparing} onClick={() => m.back()}>
          {C.back}
        </DashButton>
        <DashButton variant="filledBlue" size="large" data-testid="edit-profile-crop-use" disabled={!view || m.preparing} onClick={() => void m.useCrop()}>
          {C.crop.use}
        </DashButton>
      </div>
    </>
  );
}

/** Q1 (D11). */
function DiscardSheet({ m }: { m: EditProfileModel }) {
  return (
    <Sheet title={C.discard.title} width="small" testId="edit-profile-discard" onClose={() => m.keepEditing()}>
      <p className="sheet-text">{C.discard.text}</p>
      <div className="sheet-actions">
        <DashButton variant="tintedGray" data-autofocus data-testid="edit-profile-keep" onClick={() => m.keepEditing()}>
          {C.discard.keep}
        </DashButton>
        <DashButton variant="filledBlue" data-testid="edit-profile-discard-confirm" onClick={() => m.discard()}>
          {C.discard.confirm}
        </DashButton>
      </div>
    </Sheet>
  );
}

export function EditProfileSheet({ model, letters, username }: { model: EditProfileModel; letters: string; username: string | null }) {
  const m = useModel(model);
  if (!m.open || m.identity === null) return <GrantSheet grants={m.grants} />;
  return (
    <>
      <Sheet title={C.title} width="medium" testId="edit-profile-sheet" focusKey={m.step} closable={m.saving === null} onClose={() => m.requestClose()}>
        <div className="edit-profile-body" data-testid="edit-profile-body" data-step={m.step} data-saving={m.saving ?? undefined}>
          {m.step === "form" && <Form m={m} letters={letters} username={username} />}
          {(m.step === "url" || m.step === "gravatar") && <SourceStep m={m} kind={m.step} />}
          {m.step === "crop" && <CropStep m={m} />}
        </div>
      </Sheet>
      {m.asking && <DiscardSheet m={m} />}
      <GrantSheet grants={m.grants} />
    </>
  );
}
