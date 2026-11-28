// Tools ▸ Sign/Verify (QT-099, QT-100): dash-qt's two tabs as two cards. Sign takes an address
// (typed, or chosen from the receiving addresses, QT-097), the message, then a `signMessage`
// grant (D15); Verify shows the engine's outcome in dash-qt's words. Any edit clears the result it
// would contradict (D38).

import { useCallback, useId, useState, type ReactNode } from "react";
import type { NetworkInfo } from "../../api/generated/NetworkInfo";
import { useBackend } from "../../api/queries";
import { DashButton } from "../../components/basics";
import { CopyDashButton } from "../../components/copy";
import { useT } from "../../i18n";
import { useModel } from "../../onboarding/model";
import type { GrantModel } from "../../tools/grant";
import { SignVerifyModel } from "../../tools/signVerify";
import { AddressBookPicker } from "./AddressBook";
import { Outcome, ToolCard } from "./common";

function Field({ label, children }: { label: string; children: (id: string) => ReactNode }) {
  const id = useId();
  return (
    <div className="form-field">
      <label htmlFor={id}>{label}</label>
      {children(id)}
    </div>
  );
}

export function SignVerifyTab({ grants, walletId, network }: { grants: GrantModel; walletId: string; network: NetworkInfo }) {
  const t = useT();
  const b = useBackend();
  const [model] = useState(() => new SignVerifyModel(b, grants, walletId, network.dirName));
  useModel(model);
  const [signAddress, setSignAddress] = useState("");
  const [signMessage, setSignMessage] = useState("");
  const [verifyAddress, setVerifyAddress] = useState("");
  const [verifyMessage, setVerifyMessage] = useState("");
  const [signature, setSignature] = useState("");
  const [picking, setPicking] = useState(false);
  const chosen = useCallback(
    (address: string) => {
      setSignAddress(address);
      model.clearSign();
      setPicking(false);
    },
    [model],
  );
  const verified = model.verifyResult;

  return (
    <div className="tool-stack" data-testid="tools-signVerify">
      <ToolCard title={t("tools.signVerify.signTitle")} testId="sign-card">
        <p className="placeholder-note">{t("tools.signVerify.signIntro")}</p>
        <form
          className="tool-stack"
          onSubmit={(e) => {
            e.preventDefault();
            void model.sign(signAddress, signMessage);
          }}
        >
          <Field label={t("tools.signVerify.address")}>
            {(id) => (
              <span className="tool-inline-form">
                <input
                  id={id}
                  className="tool-field mono"
                  data-testid="sign-address"
                  value={signAddress}
                  autoComplete="off"
                  spellCheck={false}
                  placeholder={t("tools.signVerify.signAddressPlaceholder")}
                  onChange={(e) => {
                    setSignAddress(e.target.value);
                    model.clearSign();
                  }}
                />
                <DashButton variant="tintedGray" size="small" data-testid="sign-choose" onClick={() => setPicking(true)}>
                  {t("tools.signVerify.choose")}
                </DashButton>
              </span>
            )}
          </Field>
          <Field label={t("tools.signVerify.message")}>
            {(id) => (
              <textarea
                id={id}
                className="tool-field tool-message"
                data-testid="sign-message"
                value={signMessage}
                rows={4}
                placeholder={t("tools.signVerify.signMessagePlaceholder")}
                onChange={(e) => {
                  setSignMessage(e.target.value);
                  model.clearSign();
                }}
              />
            )}
          </Field>
          <div className="tool-actions">
            <DashButton type="submit" variant="filledBlue" size="small" data-testid="sign-sign" disabled={model.signing || signAddress.trim() === ""}>
              {model.signing ? t("tools.signVerify.signing") : t("tools.signVerify.signButton")}
            </DashButton>
            <DashButton
              variant="tintedGray"
              size="small"
              data-testid="sign-clear"
              onClick={() => {
                setSignAddress("");
                setSignMessage("");
                model.clearSign();
              }}
            >
              {t("tools.signVerify.clearAll")}
            </DashButton>
          </div>
          <Outcome ok={model.signResult?.ok ?? true} text={model.signResult?.text ?? null} testId="sign-result" polite reserved />
          {model.signature && (
            <Field label={t("tools.signVerify.signature")}>
              {(id) => (
                <span className="tool-inline-form">
                  <input id={id} className="tool-field mono" data-testid="sign-signature" value={model.signature} readOnly />
                  <CopyDashButton variant="tintedGray" size="small" data-testid="sign-copy" text={model.signature}>
                    {t("tools.signVerify.copy")}
                  </CopyDashButton>
                </span>
              )}
            </Field>
          )}
        </form>
      </ToolCard>
      <ToolCard title={t("tools.signVerify.verifyTitle")} testId="verify-card">
        <p className="placeholder-note">{t("tools.signVerify.verifyIntro")}</p>
        <form
          className="tool-stack"
          onSubmit={(e) => {
            e.preventDefault();
            void model.verify(verifyAddress, signature, verifyMessage);
          }}
        >
          <Field label={t("tools.signVerify.address")}>
            {(id) => (
              <input
                id={id}
                className="tool-field mono"
                data-testid="verify-address"
                value={verifyAddress}
                autoComplete="off"
                spellCheck={false}
                placeholder={t("tools.signVerify.verifyAddressPlaceholder")}
                onChange={(e) => {
                  setVerifyAddress(e.target.value);
                  model.clearVerify();
                }}
              />
            )}
          </Field>
          <Field label={t("tools.signVerify.message")}>
            {(id) => (
              <textarea
                id={id}
                className="tool-field tool-message"
                data-testid="verify-message"
                value={verifyMessage}
                rows={4}
                onChange={(e) => {
                  setVerifyMessage(e.target.value);
                  model.clearVerify();
                }}
              />
            )}
          </Field>
          <Field label={t("tools.signVerify.signature")}>
            {(id) => (
              <input
                id={id}
                className="tool-field mono"
                data-testid="verify-signature"
                value={signature}
                autoComplete="off"
                spellCheck={false}
                onChange={(e) => {
                  setSignature(e.target.value);
                  model.clearVerify();
                }}
              />
            )}
          </Field>
          <div className="tool-actions">
            <DashButton type="submit" variant="filledBlue" size="small" data-testid="verify-verify" disabled={verifyAddress.trim() === "" || signature.trim() === ""}>
              {t("tools.signVerify.verifyButton")}
            </DashButton>
            <DashButton
              variant="tintedGray"
              size="small"
              data-testid="verify-clear"
              onClick={() => {
                setVerifyAddress("");
                setVerifyMessage("");
                setSignature("");
                model.clearVerify();
              }}
            >
              {t("tools.signVerify.clearAll")}
            </DashButton>
          </div>
          <Outcome ok={!verified || model.verified} text={verified?.text ?? null} testId="verify-result" polite reserved />
        </form>
      </ToolCard>
      {picking && <AddressBookPicker walletId={walletId} network={network} purpose="receive" onChoose={chosen} onClose={() => setPicking(false)} />}
    </div>
  );
}
