//! The vault: lock state, unlocking and locking.

use dw_vault::UnlockScope;
use zeroize::Zeroizing;

use crate::dto::LockStatus;
use crate::ipc::commands;

pub(crate) const EXCLUDED: &[(&str, &str)] = &[];

commands! {
    #[facade("NetworkSession::vault", "Vault::status")]
    fn lock_status(state) -> LockStatus {
        Ok(state.session().await?.vault().status().into())
    }

    /// Full unlock with the vault passphrase. The copy this process owns is
    /// zeroed on drop; the renderer's (a DOM value, a JS string) cannot be
    /// (G-01 H-01).
    #[facade("NetworkSession::vault_op", "Vault::unlock")]
    fn unlock(state, passphrase: String) -> LockStatus {
        let passphrase = Zeroizing::new(passphrase);
        let session = state.session().await?;
        let status = session
            .vault_op(move |v| v.unlock(passphrase.as_bytes(), UnlockScope::Full))
            .await?;
        Ok(status.into())
    }

    /// The full lock (E0-04 §8.1): revokes leases and returns after their
    /// drain; `lockProgress` events report it.
    #[facade("NetworkSession::lock_vault")]
    fn lock(state) -> LockStatus {
        Ok(state.session().await?.lock_vault().await?.status.into())
    }
}
