import type { Tree } from "../core";

export const dashpay = {
  /** The DashPay card on Home and the Join page, by state. `username` is "@" and the name. */
  card: {
    error: { title: "Could not read DashPay's state", message: "Your wallet works as usual.", action: "Try again" },
    looking: { title: "Looking for your DashPay identity…" },
    notStarted: { title: "DashPay has not started", message: "It starts when the wallet connects to the Dash network." },
    unlockToLook: { title: "Unlock to look for your DashPay identity", message: "DashPay looks for your identity once the wallet is fully unlocked." },
    lookFailed: { title: "Could not finish looking for your DashPay identity", message: "Dash Wallet looks again the next time it starts." },
    unverified: { title: "Could not confirm your DashPay identity", message: "Its keys could not be checked against this wallet's recovery phrase." },
    join: { title: "Get a DashPay username", message: "Pay friends by name instead of an address.", action: "Join DashPay" },
    joinWaiting: {
      note: "Available after sync finishes",
      /** `percent` is the sync's progress, with one decimal. */
      notePercent: "Available after sync finishes · {percent}%",
    },
    watchOnly: { title: "DashPay needs this wallet's keys", message: "A watch-only wallet cannot join DashPay." },
    registering: { title: "Creating your username", message: "Your username is being created on the Dash network. It may take some time." },
    creating: "Creating – {username}",
    paused: "Registration paused",
    repair: {
      unknown: "The payment may have been sent",
      finish: "Funds locked — finishing",
      message: "Open Tools ▸ Repair to finish the transfer.",
      action: "Open Repair",
    },
    voting: {
      title: "Voting – {username}",
      deciding: "Masternode owners are deciding whether you get this username.",
      endsOn: "Results on {date}.",
      notify: "We will notify you when voting ends.",
      temporary: "Until then you are {username}.",
      action: "What is username voting?",
    },
    noUsername: { title: "Finish username registration", message: "Your identity is ready. Use its existing credits to register a username.", action: "Choose a username" },
    lowCredits: {
      title: "Your credit balance is low",
      message: "Top-up your credits to continue making changes to your profile and adding contacts",
    },
    emptyCredits: {
      title: "Your credit balance is fully depleted",
      message: "You can continue to use DashPay for payments but you cannot update your profile or add more contacts until you top up your credit balance",
    },
    buyCredits: "Buy credits",
    unlock: "Unlock",
    continue: "Continue",
    moveToCore: "Move to Dash wallet",
  },
  /** The instant username beside a contested request (iOS `CreateInstantUsernameSheet`). Quoted
   * names are bare labels. */
  instant: {
    title: "Create an instant username",
    /** `duration` is how long the vote runs ("2 weeks"). */
    offer: "Your requested username will be voted on for {duration}. For immediate contacts and payments, you can create a similar instant username.",
    offerMore: "If your request is approved, it becomes your main username and the instant one stays yours too, with your contacts and history kept. If not, keep using the instant one without interruption.",
    create: "Create instant username",
    decline: "No, thanks",
    explainer: "While “{username}” is out for a vote it does not belong to you yet and other users cannot find you by it. This username works right away and stays yours whatever the vote decides.",
    contested: "This username would also need a vote. Add a digit from 2 to 9, or use 20 characters or more.",
    same: "Your instant username must be different from the contested one",
    /** `username` is "@" and the name. */
    confirm: "You chose {username} as an instant username. It can't be changed once it's registered.",
    success: "In the meantime you are reachable at “{instant}”, which is yours to keep. If the vote awards you “{requested}”, you will be reachable at both usernames.",
    failure: "The instant username “{username}” could not be registered, so you have no username to use while the vote runs.",
    retry: "Try again",
    weeks: { one: "{count} week", other: "{count} weeks" },
    /** A network without a known vote length: the existing voting copy's wording. */
    aboutTwoWeeks: "about two weeks",
  },
  /** The own vote's card and details. Quoted names are bare labels. */
  vote: {
    instantHint: "While “{requested}” is out for a vote, you can create an instant username to use now.",
    instantAction: "Create instant username",
    instantShort: "Not enough credits for an instant username.",
    topUp: "Top up",
    wonMain: "“{username}” is now your main username. Your instant username “{instant}” stays yours too.",
  },
  /** A registration's three steps. `step` is the step's number out of three ("1/3"); `text` is the step. */
  step: {
    shown: "({step}) {text}",
    progress: "Registration progress",
    checkingInvitation: "Checking the invitation",
    payment: "Processing payment",
    identity: "Creating identity",
    username: "Registering username",
  },
  /** What a running registration waits for. */
  wait: {
    payment: "Waiting for the network to confirm the payment.",
    network: "Waiting for the Dash network. It tries again on its own.",
    sync: "Waiting for the wallet to sync.",
    startup: "Waiting for DashPay to start.",
    unlock: "Unlock the wallet to continue.",
    authorize: "Waiting for your approval to continue.",
    paymentUnknown: "Checking whether the payment from your Platform balance went through.",
    paymentUnknownShielded: "Checking whether the payment from your Shielded balance went through.",
    /** Shown beside Continue, which checks again. */
    paymentUnknownShieldedIdle: "Dash Wallet hasn't checked the payment from your Shielded balance since it reopened. Choose Continue to check it again. This doesn't spend anything.",
    paymentHeld: "Dash Wallet can't tell yet whether the payment from your Platform balance went through. It may still go through, so paying again waits until that's known. Check back later.",
    /** Shown only beside both Continue and Move to Dash wallet, which it names. */
    paymentHeldActions: "Dash Wallet can't tell yet whether the payment from your Platform balance went through. It may still go through. Choosing Continue tries the same payment again, so it can't pay twice. To free your Platform balance instead, choose Move to Dash wallet. Only one of the two can happen: if the move lands first, the registration isn't paid.",
  },
  /** Why a registration stopped. `needed` and `available` are amounts; `code` is the engine's. */
  failure: {
    fundingShort: "Not enough Dash to pay for the username.",
    fundingNeeded: "Not enough Dash to pay for the username: {needed} needed.",
    fundingNeededAvailable: "Not enough Dash to pay for the username: {needed} needed, {available} available.",
    coinsLocked: "The coins to pay with are held by another payment.",
    islockTimeout: "The network did not confirm the payment in time.",
    notSynced: "The wallet had not finished syncing.",
    grantExceeded: "The cost went above what you approved.",
    insufficientCredits: "The identity's credits do not cover the username.",
    unavailable: "The Dash network did not answer.",
    /** A paid failure of a registration from the Platform balance (C-funding F10): the engine
     * reports no amount, so the line names none. */
    refundedPlatform:
      "Dash Platform could not create the identity with this wallet's keys and kept a Platform fee and a penalty from your Platform balance for the attempt. The rest of your Platform balance was not used.",
    /** A paid failure of a registration from the Shielded balance (C-funding F12): Platform sent
     * the payment, less a penalty, to this wallet's Platform balance. */
    refundedShielded:
      "Dash Platform couldn't register the username. It kept part of the payment as a penalty and sent the rest to your Platform balance.",
    /** A registration from the Shielded balance whose coins read spent with no identity made
     * (C-funding F12): another device with the same recovery phrase may have spent them, so the
     * line claims no refund. */
    consumedShielded:
      "Dash Wallet couldn't register the username. The Shielded funds chosen to pay for it have been spent, but no username was registered. Either this payment spent them, and Dash Platform kept a penalty and sent the rest to your Platform balance, or another device using this recovery phrase spent them. Check your Platform balance before you start a new registration.",
    /** A Shielded-funded run whose readiness no longer held when it was about to pay. */
    shieldedNotReady: "Your Shielded balance can't pay for the username right now, so nothing was spent. Try again in a moment.",
    /** A Shielded payment that was refused before it was sent. */
    shieldedRefused: "The payment from your Shielded balance didn't go through, so nothing was spent. You can try again.",
    /** A run that stopped when its Platform plan changed (`registration.quote_changed` on the
     * row), whether or not its credits moved first; Confirm's start refusal has its own line. */
    quoteChanged:
      "No identity was paid for. If you chose Move to Dash wallet for this registration, those credits are in your Dash wallet now, less that move's fee. Review the cost and confirm to try again.",
    stoppedWith: "It stopped with {code}.",
  },
  /** An invitation claim's refusals. */
  refusal: {
    hasIdentity: "This wallet already has a DashPay identity. An invitation creates a first identity; it can be used with another wallet.",
    claimed: { title: "This invitation has already been used", text: "Each invitation creates one identity. Ask for a new one." },
    claimedSentence: "This invitation has already been used. Each invitation creates one identity. Ask for a new one.",
    expired: { title: "This invitation is no longer saved", text: "Open the invitation link again." },
    expiredSentence: "This invitation is no longer saved. Open the invitation link again.",
    invalid: "This invitation link is not valid",
  },
  /** The Join page. `amount` is an amount with its unit. */
  join: {
    headline: "Create username",
    pitch: "Forget about long crypto addresses, create the username, find friends and add them to your contacts",
    features: {
      usernames: { title: "Pay to usernames", text: "No more alphanumeric addresses" },
      friends: { title: "Add your friends & family", text: "Invite your family, find your friends by searching their usernames" },
      profile: { title: "Personalize profile", text: "Upload your picture, personalize your identity" },
    },
    cost: "A username costs about {amount}. Usernames the network votes on cost more; the exact cost is shown before you pay.",
    costUnknown: "Usernames the network votes on cost more than others. The exact cost is shown before you pay.",
    shortfall: "You need at least {amount} to create a username.",
    /** Neither the Dash wallet nor the Shielded balance can pay. Every placeholder is an amount
     * with its unit. */
    shortfallBoth: "You need at least {amount} to create a username. Your Dash wallet has {core}. Your Shielded balance has {shielded}.",
    /** As `shortfallBoth` in discreet mode, which shows the price but no balance. */
    shortfallBothDiscreet: "You need at least {amount} to create a username. Neither your Dash wallet nor your Shielded balance has enough.",
    create: "Create username",
    watchOnly: "A watch-only wallet cannot join DashPay. Restore the wallet from its recovery phrase to join.",
    joinedAs: "You have joined DashPay as {username}.",
    joined: "You have joined DashPay.",
    votingLink: "What is username voting?",
    faqLink: "About DashPay",
  },
  votingInfo: {
    title: "What is username voting?",
    intro: "The Dash network has to vote to approve some usernames before they are created",
    items: {
      someCases: { title: "Voting is only required in some cases", text: "Any username that has a number 2–9 or 20 or more characters will be automatically approved" },
      blocked: { title: "Some usernames can be blocked", text: "If enough of the network determines that a username is inappropriate, they can block it" },
      second: {
        title: "A second username to use meanwhile",
        text: "While the network votes you can register an instant username too, and use it until the vote ends",
      },
      notified: { title: "You will be notified", text: "When the voting ends, Dash Wallet tells you whether you got the username" },
      phrase: { title: "Keep your recovery phrase safe", text: "In case you lose your recovery phrase you will lose your right to your requested username" },
      masternodes: {
        title: "Masternode owners vote",
        text: "Each masternode can vote to approve or block a requested username. Voting from this wallet comes in a later release",
      },
    },
  },
  faq: {
    title: "About DashPay",
    headline: "Pay people, not addresses",
    pitch: "DashPay replaces long cryptic addresses with usernames. Add friends as contacts, send money to a name, and keep every payment organized by person.",
    items: {
      what: {
        question: "What is DashPay?",
        answer:
          "DashPay is Dash's social payments experience, built on Dash Platform. You register a username, add other users as contacts, and pay them by name — no more copying addresses.",
      },
      cost: {
        question: "What does it cost?",
        answer:
          "Creating a username is a one-time payment from your Dash balance; usernames the network votes on cost more. The exact cost is shown before you pay. Part of it becomes credits that pay for contact requests and profile changes. Payments to contacts cost the usual network fees.",
      },
      privacy: {
        question: "How private is DashPay?",
        answer:
          "Payments are private: the addresses you exchange with a contact travel inside an encrypted payload only the two of you can read, and are never published — so your payments aren't trivially linkable to your username. They are still regular transparent-chain payments, though: sophisticated chain analysis might leak information. Contact requests themselves are currently not private: anyone can see that two identities are connected. Your username and profile are also public on Dash Platform.",
      },
      privateRequests: {
        question: "When will contact requests become private?",
        answer:
          "Private contact requests are in development. Today the request itself is public — anyone can see that two identities are connected, even though the payment details inside it are encrypted. Once private contact requests ship, you will have the choice of having your friendship be public or private.",
      },
      bitcoin: {
        question: "How does this compare to Bitcoin in terms of privacy?",
        answer:
          "On Bitcoin there is no name layer, so people share and reuse addresses out in the open — once an address is known, everything it ever received is linkable to its owner. With DashPay your username is public, but it never points at a payment address: addresses are exchanged privately per contact and rotate, so paying by name doesn't publish where your money goes. Both are transparent chains, so chain analysis still applies to the coins themselves.",
      },
      ethereum: {
        question: "How does this compare to Ethereum accounts in terms of privacy?",
        answer:
          "An Ethereum account is one reusable address: your entire balance and payment history sit publicly under it, and a name (like an ENS domain) typically points straight at that address for anyone to resolve. DashPay is the opposite shape — the name resolves to an identity, not a payment address, and actual payment addresses stay inside encrypted contact exchanges, fresh for each contact.",
      },
      nameServices: {
        question: "What about Unstoppable Domains and similar name services?",
        answer:
          "Name services like Unstoppable Domains map a human-readable name to a fixed public address on-chain. Anyone can resolve the name and see every payment ever made to it. A DashPay username never publicly resolves to a payment address — addresses are revealed only inside the encrypted exchange with each contact, so your name and your money stay unlinked to outside observers.",
      },
      lostPhrase: {
        question: "What if I lose my recovery phrase?",
        answer:
          "Your username belongs to the identity your recovery phrase creates. Restoring the wallet from the phrase brings the username, your contacts and your payments back. Without the phrase nobody, Dash Wallet included, can recover them.",
      },
    },
  },
  /** The registration wizard. `username` is "@" and the name. */
  registration: {
    header: "Step {n} of 2 · {title}",
    /** What a screen reader hears when the screen changes: its title, then its lines. */
    announced: "{title}. {lines}",
    name: {
      step: "Choose a username",
      title: "Choose your Dash username",
      field: "Username",
      contested:
        "The Dash network must vote to approve this username before it is created. Usernames with a number 2–9, or 20 characters or more, are approved at once.",
      votingLink: "What is username voting?",
      instantTitle: "Create instant username",
      instantOnly: "An instant username needs a number 2–9 or 20 characters or more.",
      next: "Continue",
      cancel: "Cancel",
    },
    /** The "Pay with" step before the name (C-funding F19): where the registration's payment
     * comes from. */
    payWith: {
      /** The Join page's secondary link to the step. */
      link: "Pay another way",
      title: "Pay with",
      core: "Dash wallet",
      platform: "Platform balance",
      shielded: "Shielded balance",
      /** `amount` is the least the source must hold, with its unit. */
      short: "Not enough here. You need at least {amount}.",
      notOffered: "Not available on this network yet.",
      movePending: "Another Platform transfer is still being confirmed. Try again when it is done.",
      /** `time` is a clock time, HH:MM. */
      resting: "Your shielded funds arrived less than 3 hours ago. For your privacy, you can use them from {time}.",
      unsettled: "An earlier shielded payment is still being checked. Try again when it is settled.",
      /** `unsettled` while what holds the payment back is a hold: no "Try again". */
      held: "An earlier shielded payment may still go through, so part of your Shielded balance is held and can't be used yet. You can pay from Shielded once that payment goes through, or once the network shows it can no longer go through. Dash Wallet keeps checking while it's open.",
      syncing: "Your shielded balance is still syncing.",
    },
    /** "Make your username private", the step before the name (C-funding F19; the iOS and Android
     * wallets' copy, "Dash balance" read as "Dash wallet", and the Shielded note says what it hides:
     * the link between the funds and the username, which itself is public). */
    privacy: {
      title: "Make your username private",
      titleNoShielded: "Pay for your username",
      body: "To help keep your payment activity private, we recommend paying for the username from your Shielded balance",
      shieldedNote: "Helps hide the link between your funds and your username",
      coreNote: "Funds will be traceable to your username",
      advanced: "Advanced",
      shieldFirst: "Shield your funds first",
      withoutPrivacy: "Continue without privacy",
      /** The card shown in place of the sources while the Shielded balance cannot pay. */
      tip: {
        restingTitle: "Funds are resting",
        /** `time` is a clock time, HH:MM. */
        resting: "Your shielded funds are resting. They will be ready around {time} — creating your username before then links it to where the funds came from.",
        poolTitle: "Privacy pool too small",
        /** `notes` and `required` are counts of notes. */
        pool: "The shared privacy pool holds {notes} of the {required} notes a private registration needs. That count comes from everyone's shielded activity, so it grows on its own — try again later.",
        partlyTitle: "Not enough shielded funds",
        /** `have`, `need` and `more` are amounts with their unit. */
        partly: "Your Shielded balance holds {have} of the {need} a private registration needs. Add {more} more, then let it rest a few hours.",
        nothingTitle: "Privacy tip",
        /** `need` is an amount with its unit. */
        nothing: "Paying privately takes at least {need} in your Shielded balance. Funds move there in seconds, then rest a few hours before they can pay for a username.",
        waitTitle: "Shielded balance",
      },
    },
    /** The name's checklist. */
    rule: {
      minLength: "At least 3 characters",
      maxLength: "At most 23 characters",
      allowedCharacters: "Letters, numbers and hyphens only",
      noEdgeHyphen: "Must start and end with a letter or number",
      noDoubleHyphen: "No two hyphens in a row",
    },
    /** The line under the checklist. */
    availability: {
      checking: "Checking…",
      available: "Username available",
      contestOpen: "This username has already been requested, but you can request it too and let the network vote to decide if you can have it.",
      taken: "Username taken",
      locked: "This username is blocked",
      invalid: "This username is not valid",
      unknown: "Could not check availability. You can still continue.",
    },
    confirm: {
      step: "Confirm",
      title: "Confirm your username",
      message: "You have chosen {username} as your username. Please note that the username can NOT be changed once it is registered.",
      fee: "Upgrade fee",
      credits: "Part of it becomes credits that pay for contacts and profile changes.",
      contested:
        "The network votes on this username for about two weeks. If it is not approved, the payment is not returned; your identity keeps its credits and you can choose another username.",
      quoting: "Working out the cost…",
      quoteFailed: "Could not work out the cost.",
      shortfall: "You need at least {amount} to create this username.",
      /** `credits` is a number of credits. */
      fromCredits: "Paid from your identity's credits: up to {credits} credits",
      /** Paid from the Platform balance (C-funding F9). `total`, `funding` and `fee` are amounts:
       * `funding` what the identity receives, exactly; `fee` the Platform fee, about; `total` both. */
      fromPlatform: "About {total} leaves your Platform balance: {funding} for your identity and a Platform fee of about {fee}. Your identity starts with a balance of {funding}.",
      /** Paid from the Shielded balance (C-funding F20). Every placeholder is an amount with its
       * unit. */
      shielded: {
        leaves: "{denomination} leaves your Shielded balance.",
        fee: "Network fee: about {fee}.",
        starts: "Your identity starts with {start} in credits. The username and profile are paid from it, and about {left} stays as credits for contacts and profile changes.",
        /** `starts` when nothing is left after the username and profile. */
        startsNoneLeft: "Your identity starts with {start} in credits. The username and profile are paid from it.",
        change: "{change} returns to your Shielded balance after the next sync.",
        whole: "This uses your whole Shielded balance.",
      },
      accept: "I accept",
      pay: "Confirm & pay",
      register: "Confirm",
      back: "Back",
    },
    progress: {
      title: "Creating your username",
      message: "Your username {username} is being created on the Dash network. It may take some time.",
      close: "You can close this page or the app: the registration goes on, and Home shows its progress.",
      home: "Let me know when it's done",
      readFailed: "Could not read the registration's progress. It goes on; Home shows it.",
      authorize: "Paused. Confirm to finish creating {username}.",
      unlock: "Unlock the wallet to finish creating {username}.",
      finish: "Finish transfers",
    },
    recovery:
      "Your funds are safe. The payment is locked for this registration, and Dash Wallet finishes it once the network confirms it. If it does not finish, Tools ▸ Repair can finish the transfer.",
    failed: { title: "Registration failed", another: "Choose another username", startNew: "Start a new registration", reviewCost: "Review the cost" },
    stopped: { title: "Registration stopped" },
    done: {
      title: "Welcome to DashPay, {username}",
      message: "Your username is ready. Friends can now find you and pay you by name.",
      next: "What's next",
      action: "Go to wallet",
      requestSent: "A contact request was sent to your inviter.",
      requestRefused: "The contact request to your inviter could not be sent. You can add them from Contacts.",
    },
    /** The welcome's next steps. */
    welcome: {
      editProfile: { title: "Add a photo and name", text: "So friends know it's you." },
      findFriends: { title: "Find friends", text: "Search for people you know by username." },
      share: { title: "Share my username", text: "Show my QR code or copy my link." },
    },
    nameFailed: {
      title: "Your identity is ready",
      /** `why` is a sentence. */
      message: "{username} could not be registered. {why}",
      another: "Choose another username",
    },
    /** Why a name was refused for good. */
    nameFailure: {
      taken: "Username was taken by another user.",
      contestOpen: "Another user requested it, and the network is voting on it.",
      locked: "The network blocked this username.",
      invalid: "This username is not valid.",
      credits: "The identity's credits did not cover it.",
    },
    voting: {
      title: "Voting for your username has begun",
      message: "Your username {username} has been requested on the Dash network.",
      action: "Done",
      info: "What is username voting?",
    },
    wedge: {
      title: "Start a new registration?",
      text: "An earlier registration's payment could not be confirmed, and it may still appear on the network. You can start a new registration now. The earlier payment may still confirm. If it does, its funds show up as a transfer this wallet can recover later with Tools ▸ Repair ▸ Finish transfers, as a separate identity, so you may have paid for two registrations. The earlier registration can't be discarded.",
      confirm: "Start a new registration",
      cancel: "Cancel",
    },
    /** Why a start, a resume or a name was refused. `wait` is a time ("5 minutes"). */
    startError: {
      coinsLocked: "The coins to pay with are held by another payment. Try again once it ends.",
      notSynced: "Wait until the wallet has finished syncing, then try again.",
      fundingUnknownLater: "This wallet can't yet tell when the earlier registration's payment was sent. Try again later.",
      fundingUnknownWait: "The earlier registration's payment was sent less than an hour ago. Try again in {wait}.",
      hasUsername: "This wallet already has a username.",
      inProgress: "This registration's payment must be finished first. Open Repair to finish it.",
      grantExceeded: "The cost went above what you approved. Try again.",
      insufficientCredits: "The identity's credits do not cover this username.",
      taken: "Username taken. Choose another one.",
      locked: "This username is blocked. Choose another one.",
      invalid: "This username is not valid.",
      contestOpen: "The network is already voting on this username, and it can no longer be joined.",
      unreachable: "Could not reach the Dash network. Try again.",
      unlock: "Unlock the wallet and try again.",
      quoteChanged: "The cost changed. Review it and confirm again. Nothing was paid.",
    },
  },
  /** Finish transfers' report. Each value but `kind` and `kinds` is a count. */
  finished: {
    counts: "{resumed} resumed, {completed} completed, {stillPending} still pending.",
    countsUsed: "{resumed} resumed, {completed} completed, {stillPending} still pending, {alreadyUsed} already used.",
    /** `kind` is a kind of transfer ("Shielded"). */
    byKind: "{kind}: {completed} completed.",
    byKindPending: "{kind}: {completed} completed, {stillPending} still pending.",
    /** `kinds` lists the kinds of transfer ("Shielded and Platform"). */
    unread: "{kinds} transfers couldn't be checked; try again.",
    kind: { shielded: "Shielded", platformAddress: "Platform" },
  },
  /** Credits as My Profile and the top-up and withdraw sheets show them. `credits` is a number of credits. */
  credits: {
    only: "{credits} credits",
    /** What a balance buys. `requests` and `updates` are counts. */
    estimate: "≈ {requests} contact requests · {updates} profile updates",
    pendingFirst: "Finish the pending top-up first.",
    title: "Credit balance",
    unknown: "Credit balance unknown",
    unknownLine: "Dash Wallet reads it from the network.",
    updating: "Updating…",
    low: {
      title: "Your credit balance is low",
      text: "Top-up your credits to continue making changes to your profile and adding contacts",
    },
    empty: {
      title: "Your credit balance is fully depleted",
      text: "You can continue to use DashPay for payments but you cannot update your profile or add more contacts until you top up your credit balance",
    },
    topUp: "Top up",
    toppingUp: "Topping up…",
    /** A top-up that ended while the sheet was hidden. */
    unseen: "Your top-up has an update.",
    showResult: "Show",
  },
  /** My Profile. `username` is a username with its "@" ("@alice"). */
  profile: {
    loading: "Loading your profile…",
    noProfile: "You have no DashPay profile yet",
    editProfile: "Edit Profile",
    share: "Share Username",
    usernames: "Usernames",
    identity: "Identity",
    copyIdentityId: "Copy identity ID",
    noUsername: "No username",
    /** The requested name while its vote runs and no other name is held. */
    contested: "{username} · Pending",
    updating: "Updating…",
    notLoaded: "Username not loaded",
    chooseName: "Choose a username",
    /** A top-up waiting to be added. `amount` is an amount with its unit. */
    pending: {
      title: "Top-up of {amount} pending",
      unproved: "Waiting for the network to confirm the payment.",
      proved: "Ready to add.",
      unknown: "The network says this payment was already used. Check your balance.",
      finish: "Finish",
      arrived: "Credits arrived",
    },
    stuck: {
      willBeSent: "The payment is being sent",
      text: "Whether this top-up's payment reached the network is unknown. Tools ▸ Repair settles it; you can top up again once it has.",
    },
    /** The own username request's card. */
    request: {
      instantLine: "Register a second username now and use it until the vote ends.",
      won: { title: "{username} is yours", text: "The network approved your username." },
      lost: "{username} went to another user",
      lostNoWinner: "Voting on {username} ended with no winner",
      locked: { title: "{username} was blocked", text: "The network voted to block this username." },
      readFailed: "Could not read the votes.",
      retry: "Try again",
      votingLink: "What is username voting?",
      votesFor: "Votes for you",
      others: "Other requests",
      /** `others` is how many other requests there are; `count` their votes. */
      othersValue: { other: "{others}, {count} votes" },
      /** Their votes not known. */
      othersUnknown: "{others}, — votes",
      block: "Votes to block",
      abstain: "Abstained",
    },
    outcomes: {
      title: "Username voting results",
      won: "{username} — yours",
      lost: "{username} — went to another user",
      locked: "{username} — blocked",
    },
  },
  /** Edit Profile. */
  edit: {
    title: "Edit Profile",
    displayName: "Display Name",
    aboutMe: "About me",
    /** `count` is the characters typed and the limit, as one value ("12/25"). */
    count: "{count} characters",
    countOver: "{count} characters. At most {max} characters.",
    save: "Save Changes",
    saving: "Saving…",
    uploading: "Uploading your picture to the network",
    cancel: "Cancel",
    saved: "Your profile was saved.",
    empty: "A profile needs a display name, an about text or a picture.",
    picture: {
      change: "Change picture",
      chooseFile: "Choose File…",
      url: "Public URL",
      gravatar: "Gravatar",
      remove: "Remove picture",
      undo: "Undo",
      isNew: "New picture",
      isRemoved: "Picture removed",
      drop: "Drop or paste a picture here",
      noUpload: "Pictures from this computer need an upload service this version does not have yet. Use a public URL or Gravatar.",
      off: "Profile pictures are off. Turn on “Load contact pictures” in Settings to choose a new picture.",
      openSettings: "Open Settings",
    },
    url: {
      title: "Public URL",
      text: "You can specify any URL which is publicly available on the internet so users can see it on the Dash network.",
      field: "Paste your image URL",
      fetch: "Fetch Image",
      fetching: "Fetching Image",
    },
    gravatar: {
      title: "Gravatar",
      field: "Enter your Gravatar Email ID",
      placeholder: "sample@email.com",
      disclaimer:
        "Your Email is not stored in the DashPay wallet nor on any servers. It is used once to get your Gravatar account details and then discarded.",
      fetch: "Fetch Gravatar",
      fetching: "Fetching your Gravatar",
    },
    crop: {
      title: "Move and zoom your photo to find the perfect fit",
      zoom: "Zoom",
      use: "Use Photo",
      preparing: "Preparing your picture…",
      frame: "Picture crop. Drag, or use the arrow keys to move it and + or − to zoom.",
    },
    back: "Back",
    discard: {
      title: "Discard your changes?",
      text: "You made changes to your profile that are not saved.",
      keep: "Keep Editing",
      confirm: "Discard",
    },
    errors: {
      tooLarge: "This picture is too large. Choose one under 5 MB.",
      /** `max` is the largest side in pixels. */
      tooManyPixels: "This picture is too large. Choose one at most {max} pixels wide and high.",
      /** `max` is the field's limit in characters. */
      displayNameTooLong: "The display name can be at most {max} characters.",
      aboutTooLong: "About me can be at most {max} characters.",
      unsupported: "Use a PNG, JPEG or GIF picture.",
      unreadable: "This picture could not be read.",
      urlFailed: "Unable to fetch image. Please enter a valid image URL.",
      urlInvalid: "Enter the picture's https:// address.",
      gravatarFailed: "Unable to fetch your Gravatar. Please enter a valid Gravatar email ID.",
      emailInvalid: "Enter a valid email address.",
      uploadFailed: "Unable to upload your picture. Please try again.",
      uploadUnconfigured: "Uploading pictures is not available in this version.",
      candidateGone: "The picture you chose is no longer ready. Choose it again.",
      conflict: "Your profile was changed on another device since you opened it. Save again to replace it with your changes, or cancel to keep the new one.",
      conflictMatches: "Your profile was changed on another device since you opened it, and it now matches your changes.",
      unsupportedContract: "This version cannot edit profiles on the network's DashPay contract. Update Dash Wallet.",
      credits: "Not enough credits. Top up your credits and try again.",
      unavailable: "Could not reach Dash Platform. Try again.",
      unlock: "Unlock the wallet and try again.",
      context: "Waiting for the network to sync and verify this identity. Try again shortly.",
      trust: "This identity's data could not be verified against the network. Nothing was changed.",
      grant: "The cost went above what you approved.",
      dialog: "The file dialog could not be shown.",
      fileRead: "The file could not be read.",
    },
  },
  /** The top-up sheet. `amount` and `min` are amounts with their unit. */
  topUp: {
    title: "Top up credits",
    amount: {
      intro: "Credits pay for contact requests and profile changes on Dash Platform.",
      custom: "Other amount",
      field: "Amount",
      /** `unit` is the unit's name ("tDASH"). */
      fieldUnit: "Amount ({unit})",
      below: "The smallest top-up is {min}.",
      invalid: "Enter an amount.",
      first: "Enter an amount first.",
      next: "Continue",
      cancel: "Cancel",
    },
    confirm: {
      message: "You have chosen to top up your identity. The following amount will be deducted from your balance.",
      amount: "Amount",
      fee: "Top-up fee",
      total: "Total",
      /** `estimate` is what the credits buy. */
      adds: "Adds about {credits} credits: {estimate}",
      quoting: "Working out the fee…",
      back: "Back",
      pay: "Confirm & pay",
    },
    running: {
      title: "Adding credits…",
      text: "The network confirms the payment first. This can take a minute. You can close this: My Profile shows the top-up.",
    },
    done: "Done",
    close: "Close",
    /** The outcomes. `balance` is the credit balance as shown. */
    outcome: {
      added: "Credits added",
      plus: "+{credits} credits.",
      balance: "Your balance is {balance}.",
      plusBalance: "+{credits} credits. Your balance is {balance}.",
      sent: { title: "Payment sent", text: "The network has not confirmed the payment yet. Your credits are added once it does; My Profile shows the top-up until then." },
      notApplied: { title: "The credits were not added yet", text: "Your payment is safe. Finish the top-up from My Profile." },
      unknown: {
        title: "Check your credit balance",
        text: "The network says this payment was already used. If your credits went up, choose 'Credits arrived' in My Profile; otherwise Finish tries again.",
      },
    },
    /** "Pay with" (C-routes D-C3): the sources a top-up is paid from. */
    payWith: {
      title: "Pay with",
      core: "Dash wallet",
      platform: "Platform",
      shielded: "Shielded",
      nothing: "Your Platform addresses don't hold enough for a top-up and its fee.",
      max: "Max",
    },
    /** Why a "Pay with" source cannot pay, where it differs from the other sheets' reasons. */
    source: {
      shieldedNotOffered: "Paying from Shielded isn't available here yet.",
    },
    /** As `transfer.status`, for a top-up from the Platform addresses. */
    status: {
      settledUnknown: {
        title: "Settled with another payment or move",
        text: "This top-up and another payment or move from the same Platform addresses couldn't both go through, and only one did. Dash Wallet can't tell which. Check your identity's balance before topping up again.",
      },
    },
    /** A top-up from the Platform addresses. `identity` is the username or "your identity"; `amount`,
     * `total` and `fee` are amounts; `address` is the address the fee is taken from. */
    fromPlatform: {
      you: "your identity",
      confirm: "Top up {identity} with {amount} from your Platform addresses? About {total} leaves them: Dash Platform takes a fee of about {fee} from {address}.",
      fee: "Estimated Platform fee",
      pending: {
        title: "Top-up sent",
        text: "This top-up may still go through. Your Platform balance stays held until it does; don't top up from Platform again until it shows as done.",
      },
      held: {
        text: "This top-up hasn't gone through yet and still could. To free your Platform balance, move these credits to your Dash wallet; only one of the two can happen.",
        action: "Move to Dash wallet",
      },
      failed: { title: "The top-up did not go through", text: "Nothing left your Platform addresses." },
      error: {
        short: "Your Platform addresses don't hold enough for this top-up and its fee.",
        quoteChanged: "What you can pay from your Platform addresses changed since you reviewed this top-up. Review it again.",
        /** `min` is an amount. */
        below: "The smallest top-up from Platform is {min}.",
      },
    },
    /** Why a top-up was refused. `needed` and `available` are amounts; `code` is the engine's. */
    error: {
      short: "Not enough Dash.",
      needed: "Not enough Dash: {needed} needed.",
      neededAvailable: "Not enough Dash: {needed} needed, {available} available.",
      belowMin: "The smallest top-up is {min}.",
      below: "The amount is below the smallest top-up.",
      coinsLocked: "The coins to pay with are held by another payment.",
      maybeSent: "The payment may have been sent. Open Tools ▸ Repair to settle it; you can top up again once it has.",
      willBeSent: "The payment is still being sent. Try again in a minute.",
      context: "Waiting for the network to sync and verify this identity. Try again shortly.",
      trust: "This identity's data could not be verified against the network. Nothing was paid.",
      unavailable: "Dash Platform did not answer. Try again.",
      signer: "Unlock the wallet fully and try again.",
      grantExceeded: "The cost went above what you approved.",
      stoppedWith: "It stopped with {code}.",
    },
  },
  /** The withdraw sheet and My Profile's withdrawals. `amount` is an amount with its unit. */
  withdraw: {
    title: "Withdraw credits",
    action: "Withdraw",
    form: {
      intro: "Turn credits back into Dash. Dash Platform sends it to a Dash address.",
      amount: "Amount",
      /** `unit` is the unit's name ("tDASH"). */
      amountUnit: "Amount ({unit})",
      max: "Max",
      maxLine: "Everything but the network's fee.",
      to: "To",
      toHint: "Your wallet's receive address. You can paste another Dash address.",
      invalidAmount: "Enter an amount.",
      first: "Enter an amount first.",
      noCosts: "Waiting for the network's prices. Try Max, or wait a moment.",
      invalidTo: "Enter a Dash address.",
      platformTo: "This is a Dash Platform address. Withdraw to a Dash address.",
      unconfirmed: "Your last withdrawal is not on Dash Platform yet and may still go through. If you withdraw again, at most one of the two goes through.",
      next: "Continue",
      cancel: "Cancel",
    },
    confirm: {
      message: "The following credits leave your identity. Dash Platform pays the Dash to the address below.",
      withdraw: "Withdraw",
      fee: "Network fee",
      total: "Taken from credits",
      receive: "You receive about",
      to: "To",
      quoting: "Working out the fee…",
      back: "Back",
      go: "Confirm & withdraw",
    },
    /** Followed by the document id. */
    document: "Withdrawal document:",
    running: {
      title: "Withdrawing…",
      text: "Dash Platform confirms the withdrawal. You can close this: My Profile shows it.",
    },
    done: {
      title: "Withdrawal queued",
      /** `to` is a Dash address. */
      text: "{amount} is on its way to {to}. Dash Platform sends it to the Dash chain, usually within minutes, or hours later when Platform's daily withdrawal limit is reached.",
      balance: "Your credit balance is {balance}.",
      landed: "Your credit balance updates shortly.",
    },
    recent: {
      title: "Withdrawals",
      row: "Withdrawal of {amount}",
      rowUnknown: "Withdrawal of an unknown amount",
      unconfirmed: "Withdrawal not on Dash Platform yet",
      /** Followed by the document id. */
      unconfirmedText: "It may have been sent, and shows here once Dash Platform takes it. Document:",
      status: {
        pending: "Pending",
        complete: "Sent",
        failed: "Failed: the credits were not returned",
        unknown: "Status unknown",
      },
    },
    unseen: "Your withdrawal has an update.",
    show: "Show",
    ok: "Done",
    close: "Close",
    /** Why a withdrawal was refused. `min`, `max`, `needed` and `available` are credit figures as shown. */
    error: {
      range: "Withdraw between {min} and {max}.",
      rangeUnknown: "Dash Platform does not take a withdrawal of this amount.",
      credits: "Not enough credits: {needed} needed with the fee, {available} available.",
      creditsShort: "Not enough credits.",
      keysMissing: "This identity has no transfer key in this wallet, so it cannot withdraw.",
      address: "Enter a Dash address of this network.",
      maybeSent: "It may have been sent. Check your withdrawals in My Profile before trying again.",
      landed: "Your last withdrawal went through after all, so this one does not. Withdraw again only if you want a second one.",
      context: "Waiting for the network to sync and verify this identity. Try again shortly.",
      trust: "This identity's data could not be verified against the network. Nothing was withdrawn.",
      unavailable: "Dash Platform did not answer. Try again.",
      signer: "Unlock the wallet fully and try again.",
      grantExceeded: "The cost went above what you approved.",
      stoppedWith: "It stopped with {code}.",
    },
  },
  /** An invitation's claim. `username` is "@" and the name; `who` is that, or an identity id. */
  claim: {
    title: { welcome: "You have been invited to DashPay", join: "DashPay invitation" },
    checking: "Checking the invitation…",
    checkFailed: "Could not check the invitation. Your wallet works as usual.",
    wrongNetwork: "This invitation is for another Dash network than {network}",
    assetLockNotFound: "The network does not know this invitation's funding yet. Try again later.",
    invited: {
      from: "Invited by",
      checked: "Dash Wallet checked that {username} is a DashPay username. It cannot check who sent you this link.",
      unnamed: "This invitation does not name who sent it.",
    },
    paid: "Paid by the invitation: nothing is paid from this wallet.",
    confirm: {
      recipient: "A contact request will go to {who}. It carries your new identity's extended public key for payments between you, so {who} can see the payments you make to each other.",
      recipientId: "Their identity",
      copyIdentityId: "Copy identity ID",
      request: "Send a contact request to {who}",
      claim: "Claim invitation",
    },
    contestedRefused: "This invitation cannot pay for a username the network votes on. Use a number 2–9, or 20 characters or more.",
    notCovered: "This invitation does not cover this username.",
    paste: {
      open: "Have an invitation?",
      title: "Open an invitation",
      field: "Invitation link",
      refused: "This is not a DashPay invitation link.",
      cancel: "Cancel",
      ok: "Open invitation",
    },
    kept: "This claim may already be on the network, so it is not cancelled. Continue to finish it.",
    accept: "Accept invitation",
    notNow: "Not now",
    dismiss: "Dismiss invitation",
    retry: "Try again",
  },
  /** "Sign in to an app" (App Connect): the sheet a `dashpay://connect` request opens. `label` is
   * the app's own name for itself, never verified; `host` is where the app's return link goes. */
  connect: {
    title: "Sign in to an app",
    loading: "Checking this request with Platform…",
    app: {
      claim: "“{label}” (name given by the app, not verified)",
      unverified: "Dash Wallet can't check which app sent this request. Approve it only if you just started signing in on that app's own website.",
      id: "App ID",
      copyId: "Copy App ID",
    },
    returnsTo: "Returns you to {host}",
    signInAs: "Sign in as",
    asks: "The app asks for:",
    grant: {
      contract: "A key for the app's contract",
      docType: "A key for “{type}” documents",
      profile: "A key that can edit your DashPay profile",
      encryption: "An encryption key for the app",
      /** `count` is how many contracts the group has now. */
      group: "A key for the app's contracts ({count}). The app's owner can add contracts to this group later, and this key will work for those too.",
      groupAdmins: "A key for the app's contracts ({count}). The app's owner and admins can add contracts to this group later, and this key will work for those too.",
    },
    /** The group row's details: who controls the group and what is in it now. */
    group: {
      owner: "Owner",
      admins: "Admins",
      contracts: "Contracts",
      documentTypes: "Document types",
      tokens: "Tokens",
      none: "None",
    },
    /** Under a CRITICAL binding that can reach tokens: a group, or a contract with tokens or one an
     * update could add tokens to. Only CRITICAL keys can sign token transitions. */
    critical: "This key has critical access: it can do anything these contracts let your identity do, including sending the app's tokens your identity holds. The funding limit doesn't cap tokens.",
    /** Under a CRITICAL binding that can't reach tokens: a read-only contract without tokens, or one document type. */
    criticalNoTokens: "This key has critical access: within what it covers, it can do anything your identity can.",
    /** Under a binding on the app's contracts, a document type or a group, unless the critical warning
     * names tokens already: a document write can carry a token cost the identity pays. */
    tokenCost: "Saving documents with this key can also spend tokens your identity holds, if the contract charges tokens for them. The funding limit doesn't apply to tokens.",
    cannot: "It can't spend Dash from this wallet, withdraw credits, transfer credits to anyone or change your other keys. It can spend your identity's credits up to the funding limit.",
    limit: "Funding limit (Dash)",
    limitHint: "The most this app can spend from your identity's credits.",
    /** `max` is the most the wallet lets an approval set, in Dash. */
    limitInvalid: "Enter an amount of Dash more than 0 and at most {max}, like 0.5.",
    expiry: "Access ends",
    expiryValue: "In {days} days",
    fee: "Adding these keys costs about {amount} in credits.",
    approve: "Approve",
    deny: "Deny",
    close: "Close",
    step: {
      keys: "Adding keys to your identity…",
      confirm: "Waiting for Platform…",
      send: "Sending to the app…",
    },
    done: "You're signed in to “{label}”.",
    denied: "You declined this sign-in request. Nothing was added to your identity.",
    /** Declined after an approval of this request stopped before its response was sent. */
    deniedAfterApprove: "You declined this sign-in request. Keys from your earlier approval may already be on your identity, and the app may already have them.",
    return: "Return to {host}",
    retrySend: "Retry sending",
    /** Under a quote that failed: quotes the same choice again. */
    retryQuote: "Try again",
    sendFailed: "The keys were added to your identity, but the app may not have received them yet. Retry sending, or return to the app and start again.",
    planChanged: "Something this request covers changed on Dash Platform since you opened it. Check it again before you approve.",
    err: {
      network: "This request is for {requested}, but this wallet uses {current}.",
      mainnetOff: "This version can sign in to apps on test networks only.",
      unsupported: "This app uses a sign-in method this version of Dash Wallet doesn't support.",
      invalid: "This sign-in request can't be read. Ask the app for a new one.",
      newer: "This link needs a newer version of Dash Wallet.",
      /** A new request while every request kept is on screen or being approved. */
      busy: "Dash Wallet already has too many sign-in requests in progress. Finish or close one, then start signing in again from the app.",
      stale: "This sign-in request has expired. Ask the app for a new one.",
      protocol: "This network doesn't support app sign-in yet.",
      noIdentity: "You need a DashPay username to sign in to apps. Create one in DashPay first.",
      hint: "This sign-in is for an identity that isn't in this wallet.",
      owner: "Dash Wallet refused this request: the app asked for keys covering contracts it doesn't own.",
      credits: "Your identity needs {amount} more in credits to add these keys. Top it up, then try again.",
      answered: "You've already answered this request. Ask the app for a new one.",
      check: "Couldn't check this request with Platform. Try again.",
      failed: "Sign-in didn't finish: {detail}",
      returnFailed: "Dash Wallet couldn't open {host}. Switch back to the app yourself.",
    },
    /** After `err.failed`, only where the wallet read the identity back: never after `notConfirmed`. */
    failed: {
      added: "The keys were added to your identity.",
    },
    /** `err.failed`'s `detail`, by where the approval stopped. */
    failure: {
      keyConflict: "a key for this app is already on your identity, with different limits.",
      notConfirmed: "Dash Platform didn't confirm the new keys in time. They may still be added; if you try again, Dash Wallet checks your identity first.",
      noResponseKey: "your identity has no key that can sign the answer to the app.",
      nothingToSend: "Dash Wallet had nothing to send back to the app. Start again from the app; that adds new keys to your identity, for a small fee in credits.",
      scopeChanged: "the app changed its contracts or keys on Dash Platform after your keys were added.",
      unverifiedKey: "Dash Wallet couldn't confirm which keys on Dash Platform belong to this app, so it didn't disable any.",
      feeTooHigh: "the Dash Platform fee to disable these keys is higher than Dash Wallet's limit.",
      disconnectConflict: "this identity changed on Dash Platform before the earlier disconnect finished.",
      pendingFeeTooHigh: "the Dash Platform fee to resend this disconnect is above Dash Wallet's limit.",
      pendingWrite: "another change on this identity hasn't settled on Dash Platform yet.",
      unknown: "something went wrong on the way.",
    },
    /** Settings ▸ Advanced ▸ Connected apps and the Disconnect confirmation sheet. */
    connected: {
      title: "Connected apps",
      intro: "Apps you signed in to with {who} from this wallet. Disconnecting an app disables its keys on Dash Platform, so it can't sign for {who} with them anymore.",
      introSelf: "Apps you signed in to with your identity from this wallet. Disconnecting an app disables its keys on Dash Platform, so it can't sign for your identity with them anymore.",
      refresh: "Refresh",
      checking: "Checking…",
      loading: "Checking connected apps on Dash Platform…",
      empty: "No connected apps.",
      emptySub: "Apps you sign in to with DashPay Connect show here.",
      loadError: "Dash Wallet couldn't load connected apps from Dash Platform.",
      tryAgain: "Try again",
      returnsTo: "Returns to",
      keys: "Keys",
      keyId: "Key {id}",
      fundingLimit: "Funding limit",
      approved: "Approved",
      expiry: "Access ends",
      expiryPast: "Access ended {date}",
      disconnect: "Disconnect",
      disconnected: "Disconnected “{label}”.",
      disconnectedResponsePending: "Disconnected “{label}”. Its session keys are disabled, but a sign-in response was still in flight, so a key may already have reached the app.",
      disconnectedPartial: "{disabled} of {total} keys disabled. Disconnect again to finish.",
      responsePending: "A sign-in response was still in flight, so a key may already have reached the app.",
      checkAttempt: "Check earlier attempt",
      checkAttemptBody: "Dash Wallet can check the earlier attempt without sending an update or charging a fee.",
      sheetTitle: "Disconnect this app?",
      sheetBody: "Dash Wallet disables this app's keys on Dash Platform, so the app can no longer use them to sign for {who}. Anything it already did with them stays as it is.",
      sheetBodySelf: "Dash Wallet disables this app's keys on Dash Platform, so the app can no longer use them to sign for your identity. Anything it already did with them stays as it is.",
      keysLabel: "Keys to disable",
      fee: "Disabling these keys costs about {amount} in credits.",
      priorChargeKnown: "An earlier disconnect attempt couldn't finish after another change to this identity, and up to {amount} in credits were debited. Disconnecting again sends a new paid update.",
      priorChargePossible: "An earlier disconnect attempt couldn't finish after another change to this identity and may already have been charged a Dash Platform fee. Disconnecting again sends a new paid update.",
      lowCredits: "This identity needs {amount} more in credits to disable these keys. Top up its credits, then try again.",
      busy: "Disabling keys on Dash Platform…",
      cancel: "Cancel",
      err: {
        notConfirmed: "Dash Platform didn't confirm the change, so the keys may already be disabled. Try again: Dash Wallet checks first, so you don't pay twice.",
        unverifiedKey: "Dash Wallet couldn't confirm which keys on Dash Platform belong to this app, so it didn't disable any. Nothing was changed, and the app can still use its keys. To get help, choose Report a problem in Settings ▸ About.",
        feeTooHigh: "The Dash Platform fee to disable these keys is higher than Dash Wallet's limit, so nothing was changed. Try again later.",
        grantExceeded: "The Dash Platform fee changed while disconnecting, so review the updated fee and try again.",
        disconnectConflict: "This identity changed on Dash Platform before the earlier disconnect finished, so its keys are still active. Review the updated fee and disconnect again.",
        pendingFeeTooHigh: "The Dash Platform fee to resend this disconnect is above Dash Wallet's limit, so it wasn't resent. The earlier attempt may still be in flight — refresh in a moment to check.",
        pendingWrite: "Another change on this identity hasn't settled on Dash Platform yet. Wait for it to finish, then try again.",
        generic: "Dash Wallet couldn't disconnect this app. Try again.",
        genericUnsent: "Dash Wallet couldn't disconnect this app. Try again. Nothing was changed.",
      },
    },
  },
} as const satisfies Tree;
