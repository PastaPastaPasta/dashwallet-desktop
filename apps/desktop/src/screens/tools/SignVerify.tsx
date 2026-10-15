// Tools ▸ Sign/Verify (QT-099, QT-100): dash-qt's two tabs as two cards. Sign takes an address
// (typed, or chosen from the receiving addresses, QT-097), the message, then a `signMessage`
// grant (D15); Verify shows the engine's outcome in dash-qt's words. Any edit clears the result it
// would contradict (D38).

import { useCallback, useId, useState, type ReactNode } from "react";
import type { NetworkInfo } from "../../api/generated/NetworkInfo";
import { useBackend } from "../../api/queries";
import { DashButton } from "../../components/basics";
import { copyWithToast } from "../../components/toast";
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
      <ToolCard title="Sign Message" testId="sign-card">
        <p className="placeholder-note">
          You can sign messages with your addresses to prove you can receive Dash sent to them. Be careful not to sign anything vague or random, as
          phishing attacks may try to trick you into signing your identity over to them. Only sign fully-detailed statements you agree to.
        </p>
        <form
          className="tool-stack"
          onSubmit={(e) => {
            e.preventDefault();
            void model.sign(signAddress, signMessage);
          }}
        >
          <Field label="Address">
            {(id) => (
              <span className="tool-inline-form">
                <input
                  id={id}
                  className="tool-field mono"
                  data-testid="sign-address"
                  value={signAddress}
                  autoComplete="off"
                  spellCheck={false}
                  placeholder="The Dash address to sign the message with"
                  onChange={(e) => {
                    setSignAddress(e.target.value);
                    model.clearSign();
                  }}
                />
                <DashButton variant="tintedGray" size="small" data-testid="sign-choose" onClick={() => setPicking(true)}>
                  Choose…
                </DashButton>
              </span>
            )}
          </Field>
          <Field label="Message">
            {(id) => (
              <textarea
                id={id}
                className="tool-field tool-message"
                data-testid="sign-message"
                value={signMessage}
                rows={4}
                placeholder="Enter the message you want to sign here"
                onChange={(e) => {
                  setSignMessage(e.target.value);
                  model.clearSign();
                }}
              />
            )}
          </Field>
          {model.signature && (
            <Field label="Signature">
              {(id) => (
                <span className="tool-inline-form">
                  <input id={id} className="tool-field mono" data-testid="sign-signature" value={model.signature} readOnly />
                  <DashButton variant="tintedGray" size="small" data-testid="sign-copy" onClick={() => void copyWithToast(model.signature)}>
                    Copy
                  </DashButton>
                </span>
              )}
            </Field>
          )}
          {model.signResult && <Outcome ok={model.signResult.ok} text={model.signResult.text} testId="sign-result" />}
          <div className="tool-actions">
            <DashButton type="submit" variant="filledBlue" size="small" data-testid="sign-sign" disabled={model.signing || signAddress.trim() === ""}>
              {model.signing ? "Signing…" : "Sign Message"}
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
              Clear All
            </DashButton>
          </div>
        </form>
      </ToolCard>
      <ToolCard title="Verify Message" testId="verify-card">
        <p className="placeholder-note">
          Enter the receiver's address, message (ensure you copy line breaks, spaces, tabs, etc. exactly) and signature below to verify the message. Be
          careful not to read more into the signature than what is in the signed message itself, to avoid being tricked by a man-in-the-middle attack.
          Note that this only proves the signing party receives with the address, it cannot prove sendership of any transaction!
        </p>
        <form
          className="tool-stack"
          onSubmit={(e) => {
            e.preventDefault();
            void model.verify(verifyAddress, signature, verifyMessage);
          }}
        >
          <Field label="Address">
            {(id) => (
              <input
                id={id}
                className="tool-field mono"
                data-testid="verify-address"
                value={verifyAddress}
                autoComplete="off"
                spellCheck={false}
                placeholder="The Dash address the message was signed with"
                onChange={(e) => {
                  setVerifyAddress(e.target.value);
                  model.clearVerify();
                }}
              />
            )}
          </Field>
          <Field label="Message">
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
          <Field label="Signature">
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
          {verified && <Outcome ok={model.verified} text={verified.text} testId="verify-result" />}
          <div className="tool-actions">
            <DashButton type="submit" variant="filledBlue" size="small" data-testid="verify-verify" disabled={verifyAddress.trim() === "" || signature.trim() === ""}>
              Verify Message
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
              Clear All
            </DashButton>
          </div>
        </form>
      </ToolCard>
      {picking && <AddressBookPicker walletId={walletId} network={network} purpose="receive" onChoose={chosen} onClose={() => setPicking(false)} />}
    </div>
  );
}
