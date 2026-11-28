//! Rows the gift card feature keeps in a wallet's app.sqlite scope: the
//! wallet's backup carries them, a restore puts them back and removing the
//! wallet (Delete All included) deletes them. The app seals each value to
//! the wallet's seed before it is written here.

use crate::{EngineError, NetworkSession, WalletId};

/// Every gift card row's key starts with this.
pub const GIFT_CARD_PREFIX: &str = "giftcards.";

impl NetworkSession {
    /// `wallet`'s gift card rows, by key.
    pub async fn gift_card_rows(
        &self,
        wallet: WalletId,
    ) -> Result<Vec<(String, String)>, EngineError> {
        let _op = self.enter().await?;
        self.wallet_info(&wallet)?;
        let scope = wallet.to_string();
        self.appdb_op(move |db| db.settings_with_prefix(&scope, GIFT_CARD_PREFIX))
            .await
    }

    /// Writes (`Some`) or deletes (`None`) gift card rows of registered
    /// wallets, durably and all in one transaction.
    pub async fn write_gift_card_rows(
        &self,
        writes: Vec<(WalletId, String, Option<String>)>,
    ) -> Result<(), EngineError> {
        let _op = self.enter().await?;
        for (wallet, key, _) in &writes {
            if !key.starts_with(GIFT_CARD_PREFIX) || key.len() > 256 {
                return Err(EngineError::InvalidArgument("not a gift card row".into()));
            }
            self.wallet_info(wallet)?;
        }
        self.appdb_op(move |db| {
            let scopes: Vec<String> = writes.iter().map(|(w, _, _)| w.to_string()).collect();
            let rows: Vec<(&str, &str, Option<&str>)> = writes
                .iter()
                .zip(&scopes)
                .map(|((_, key, value), scope)| (scope.as_str(), key.as_str(), value.as_deref()))
                .collect();
            db.set_scoped_settings_durable(&rows)
        })
        .await
    }
}
