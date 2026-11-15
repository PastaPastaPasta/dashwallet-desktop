// R9's next steps (PRODUCT #4): where "Add a photo and name" and "Find friends" lead. "Share my
// username" stays on the page, in a sheet with Add contact's My QR.

import type { IdentitySummary } from "../api/generated/IdentitySummary";
import { navigate } from "../app/ui";
import { showToast } from "../components/toast";
import { editProfileFor, type EditProfileApi } from "./editProfile";

/** My Profile with Edit Profile open; only the page while the main identity is unread. */
export function openEditProfile(api: EditProfileApi, walletId: string, main: IdentitySummary | undefined) {
  navigate("profile");
  if (main) editProfileFor(api, walletId, showToast).begin(main.identity, main.profile);
}

/** Contacts with Add contact open on Search. */
export function findFriends(walletId: string) {
  navigate("contacts", { contact: null, contactPrefill: null, addContact: { walletId, query: "", hit: null } });
}
