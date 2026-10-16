// The voting info (IOS-134, AND-071) and the DashPay FAQ (IOS-129) sheets (DP1-08), opened from
// the Join page and the voting card. Copy is in dashpay/status.ts.

import { Sheet } from "../../components/sheet";
import { FAQ, VOTING_INFO } from "../../dashpay/status";

export function VotingInfoSheet({ onClose }: { onClose: () => void }) {
  return (
    <Sheet title={VOTING_INFO.title} width="medium" testId="voting-info" onClose={onClose}>
      <p className="about-dashpay-intro">{VOTING_INFO.intro}</p>
      <ul className="about-dashpay-list" role="list">
        {VOTING_INFO.items.map((item) => (
          <li key={item.title}>
            <h3 className="about-dashpay-item-title">{item.title}</h3>
            <p className="about-dashpay-item-text">{item.text}</p>
          </li>
        ))}
      </ul>
    </Sheet>
  );
}

export function DashPayFaqSheet({ onClose }: { onClose: () => void }) {
  return (
    <Sheet title={FAQ.title} width="medium" testId="dashpay-faq" onClose={onClose}>
      <h3 className="about-dashpay-headline">{FAQ.headline}</h3>
      <p className="about-dashpay-intro">{FAQ.pitch}</p>
      <div className="about-dashpay-faq">
        {FAQ.items.map((item) => (
          // Native disclosure: keyboard and screen readers get it for free.
          <details key={item.question} className="about-dashpay-question" data-testid="dashpay-faq-item">
            <summary>{item.question}</summary>
            <p className="about-dashpay-item-text">{item.answer}</p>
          </details>
        ))}
      </div>
    </Sheet>
  );
}
