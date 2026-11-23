import type { Tree } from "../core";

export const notifications = {
  title: "Notifications",
  loading: "Loading…",
  loadingPage: "Loading notifications…",
  startingDashPay: "Starting DashPay…",
  emptyPopover: "No new notifications",
  seeAll: "See all notifications",
  showMore: "Show more",
  tryAgain: "Try again",
  readError: "Could not read the notifications",
  readErrorDetail: "Could not read the notifications: {error}",
  enableFirst: "Enable DashPay first. Contacts offers it.",
  moreRequests: { one: "{count} more request", other: "{count} more requests" },
  moreEvents: { one: "{count} more", other: "{count} more" },
  section: {
    payments: "Payments",
    requests: "Contact requests",
    new: "New",
    earlier: "Earlier",
  },
  received: {
    discreet: "Received a payment",
    amount: "Received {amount}",
  },
  event: {
    someone: "Someone",
    requestReceived: "{who} sent you a contact request",
    requestAccepted: "{who} accepted your contact request",
    contactEstablished: "You and {who} are now contacts",
    paymentReceived: "{who} sent you a payment",
    usernameRegistered: "Your username {username} is registered",
    contestWon: "You won the vote for {username}",
    contestLost: "You lost the vote for {username}",
    contestLocked: "The vote for {username} ended locked",
  },
} as const satisfies Tree;
