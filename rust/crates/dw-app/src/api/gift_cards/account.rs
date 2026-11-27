use zeroize::Zeroizing;

use super::piggy::Piggy;
use super::store::{Account, Secrets, Token};
use super::{GiftAccount, GiftSignup, fail, now};
use crate::error::AppError;

const PENDING: &str = "piggy-pending";
const ACCOUNT: &str = "piggy-account";
const ACCESS: &str = "piggy-access";

impl Secrets {
    pub async fn account_status(&self) -> Result<GiftAccount, AppError> {
        if let Some(pending) = self.load::<Account>(PENDING).await? {
            return Ok(GiftAccount::CodeSent {
                email: pending.signup.email,
            });
        }
        let Some(account) = self.load::<Account>(ACCOUNT).await? else {
            return Ok(GiftAccount::SignedOut);
        };
        let signed_in = self
            .load::<Token>(ACCESS)
            .await?
            .is_some_and(|token| token.account == account.key());
        Ok(if signed_in {
            GiftAccount::SignedIn {
                email: account.signup.email,
            }
        } else {
            GiftAccount::CanSignIn {
                email: account.signup.email,
            }
        })
    }

    pub async fn signup(&self, piggy: &Piggy, info: GiftSignup) -> Result<GiftAccount, AppError> {
        let info = info.checked()?;
        if self.load::<Token>(ACCESS).await?.is_some() {
            return Err(fail("Sign out of PiggyCards before changing accounts."));
        }
        let created = piggy.signup(&info).await?;
        if created.user_id.trim().is_empty() || created.user_id.len() > 128 {
            return Err(fail("PiggyCards did not return a user ID."));
        }
        let account = Account {
            signup: info,
            user_id: created.user_id,
            password: created.generated_password,
        };
        if account.password.is_some() {
            self.activate(piggy, account).await
        } else {
            self.save(PENDING, &account).await?;
            Ok(GiftAccount::CodeSent {
                email: account.signup.email,
            })
        }
    }

    pub async fn verify_email(
        &self,
        piggy: &Piggy,
        code: Zeroizing<String>,
    ) -> Result<GiftAccount, AppError> {
        let mut pending: Account = self
            .load(PENDING)
            .await?
            .ok_or_else(|| fail("Request an email code first."))?;
        let code = code.trim();
        if !(4..=12).contains(&code.len()) || !code.bytes().all(|b| b.is_ascii_digit()) {
            return Err(fail("Enter the code from your PiggyCards email."));
        }
        pending.password = Some(piggy.verify(&pending.signup.email, code).await?);
        self.activate(piggy, pending).await
    }

    async fn activate(&self, piggy: &Piggy, account: Account) -> Result<GiftAccount, AppError> {
        self.save(ACCOUNT, &account).await?;
        self.delete(PENDING).await?;
        self.login(piggy, &account).await?;
        Ok(GiftAccount::SignedIn {
            email: account.signup.email,
        })
    }

    async fn login(&self, piggy: &Piggy, account: &Account) -> Result<Token, AppError> {
        let password = account
            .password
            .as_deref()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| fail("Verify your PiggyCards email before signing in."))?;
        let (access, expires_in) = piggy.login(&account.user_id, password).await?;
        let token = Token {
            account: account.key(),
            access,
            expires_at: now().saturating_add(expires_in),
        };
        self.save(ACCESS, &token).await?;
        Ok(token)
    }

    pub async fn sign_in(&self, piggy: &Piggy) -> Result<GiftAccount, AppError> {
        let account: Account = self
            .load(ACCOUNT)
            .await?
            .ok_or_else(|| fail("Set up a PiggyCards account first."))?;
        self.delete(PENDING).await?;
        self.login(piggy, &account).await?;
        Ok(GiftAccount::SignedIn {
            email: account.signup.email,
        })
    }

    pub async fn sign_out(&self) -> Result<GiftAccount, AppError> {
        self.delete(ACCESS).await?;
        self.delete(PENDING).await?;
        self.account_status().await
    }

    pub async fn session(&self, piggy: &Piggy) -> Result<(Account, Token), AppError> {
        let signed_out = || fail("Sign in to PiggyCards first.");
        let account: Account = self.load(ACCOUNT).await?.ok_or_else(signed_out)?;
        let mut token: Token = self
            .load::<Token>(ACCESS)
            .await?
            .filter(|token| token.account == account.key())
            .ok_or_else(signed_out)?;
        if token.expires_at.saturating_sub(15) <= now() {
            token = self.login(piggy, &account).await?;
        }
        Ok((account, token))
    }
}
