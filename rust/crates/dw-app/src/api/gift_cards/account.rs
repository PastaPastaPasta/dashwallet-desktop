use zeroize::Zeroizing;

use super::store::{Account, Book, Change};
use super::{Context, GiftAccount, GiftSignup, GiftStoredAccount, Token, fail};
use crate::error::AppError;

impl Context<'_> {
    fn signed_in(&self, account: &str) -> bool {
        self.cards.token(self.store.wallet(), account).is_some()
    }

    pub async fn account_status(&self) -> Result<GiftAccount, AppError> {
        Ok(self.status_of(&self.store.read().await?))
    }

    fn status_of(&self, book: &Book) -> GiftAccount {
        if let Some(pending) = &book.pending {
            return GiftAccount::CodeSent {
                email: pending.signup.email.clone(),
            };
        }
        let Some(account) = book.active_account() else {
            return GiftAccount::SignedOut;
        };
        let email = account.signup.email.clone();
        if self.signed_in(&account.key()) {
            GiftAccount::SignedIn { email }
        } else {
            GiftAccount::CanSignIn { email }
        }
    }

    pub async fn stored_accounts(&self) -> Result<Vec<GiftStoredAccount>, AppError> {
        let book = self.store.read().await?;
        let active = book.active.as_deref();
        Ok(book
            .accounts
            .iter()
            .map(|a| {
                let id = a.key();
                GiftStoredAccount {
                    active: active == Some(id.as_str()),
                    email: a.signup.email.clone(),
                    id,
                }
            })
            .collect())
    }

    pub async fn signup(&self, info: GiftSignup) -> Result<GiftAccount, AppError> {
        let info = info.checked()?;
        let book = self.store.read().await?;
        if book.active.as_deref().is_some_and(|key| self.signed_in(key)) {
            return Err(fail("Sign out of PiggyCards before changing accounts."));
        }
        let created = self.piggy.signup(&info).await?;
        if created.user_id.trim().is_empty() || created.user_id.len() > 128 {
            return Err(fail("PiggyCards did not return a user ID."));
        }
        let account = Account {
            signup: info,
            user_id: created.user_id,
            password: created.generated_password,
        };
        if account.password.is_some() {
            self.activate(account).await
        } else {
            self.store.write(&[Change::Pending(Some(&account))]).await?;
            Ok(GiftAccount::CodeSent {
                email: account.signup.email,
            })
        }
    }

    pub async fn verify_email(&self, code: Zeroizing<String>) -> Result<GiftAccount, AppError> {
        let mut pending = (self.store.read().await?.pending)
            .ok_or_else(|| fail("Request an email code first."))?;
        let code = code.trim();
        if !(4..=12).contains(&code.len()) || !code.bytes().all(|b| b.is_ascii_digit()) {
            return Err(fail("Enter the code from your PiggyCards email."));
        }
        pending.password = Some(self.piggy.verify(&pending.signup.email, code).await?);
        self.activate(pending).await
    }

    async fn activate(&self, account: Account) -> Result<GiftAccount, AppError> {
        self.store
            .write(&[
                Change::Account(&account),
                Change::Pending(None),
                Change::Active(Some(account.key())),
            ])
            .await?;
        self.login(&account).await?;
        Ok(GiftAccount::SignedIn {
            email: account.signup.email,
        })
    }

    async fn login(&self, account: &Account) -> Result<Zeroizing<String>, AppError> {
        let password = account
            .password
            .as_deref()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| fail("Verify your PiggyCards email before signing in."))?;
        let (access, expires_in) = self.piggy.login(&account.user_id, password).await?;
        let token = Token::new(access, expires_in);
        let access = token.access.clone();
        self.cards.keep_token(self.store.wallet(), account.key(), token);
        Ok(access)
    }

    /// Signs in to the active account, or to `account` (one of this
    /// wallet's stored accounts), which becomes the active one.
    pub async fn sign_in(&self, account: Option<String>) -> Result<GiftAccount, AppError> {
        let book = self.store.read().await?;
        let key = account
            .or(book.active.clone())
            .ok_or_else(|| fail("Set up a PiggyCards account first."))?;
        let account = book
            .account(&key)
            .ok_or_else(|| fail("Set up a PiggyCards account first."))?;
        self.store
            .write(&[Change::Pending(None), Change::Active(Some(key))])
            .await?;
        self.login(account).await?;
        Ok(GiftAccount::SignedIn {
            email: account.signup.email.clone(),
        })
    }

    pub async fn sign_out(&self) -> Result<GiftAccount, AppError> {
        self.cards.forget_tokens(self.store.wallet());
        self.store.write(&[Change::Pending(None)]).await?;
        self.account_status().await
    }

    /// The active account and an access token for it.
    pub async fn active_session(&self) -> Result<(Account, Zeroizing<String>), AppError> {
        let book = self.store.read().await?;
        let account = book
            .active_account()
            .filter(|a| self.signed_in(&a.key()))
            .ok_or_else(|| fail("Sign in to PiggyCards first."))?
            .clone();
        let access = self.access(&account).await?;
        Ok((account, access))
    }

    /// An access token for `account`, signing in again with its stored
    /// password when the last one expired or this process has none: an
    /// order of an account that is not the active one stays reachable.
    pub async fn access(&self, account: &Account) -> Result<Zeroizing<String>, AppError> {
        match self.cards.token(self.store.wallet(), &account.key()) {
            Some(access) => Ok(access),
            None => self.login(account).await,
        }
    }
}
