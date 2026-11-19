//! The update check's error (PK-03a). The strings are the engine's
//! `code()`s; `tests::codes_match_the_engine` holds them equal.

use dw_engine::updates::UpdatesError;

use super::{AppError, codes};

codes! {
    UpdatesCode {
        /// A check while the update check is off, or turned off while it ran:
        /// no request is made, nothing it brought is kept.
        Off = "updates.off",
        /// `updates.json` could not be written.
        Storage = "updates.storage",
    }
}

impl From<UpdatesError> for AppError {
    fn from(e: UpdatesError) -> Self {
        let message = e.to_string();
        let code = match e {
            UpdatesError::Off => UpdatesCode::Off,
            UpdatesError::Storage(_) => UpdatesCode::Storage,
        };
        AppError::new(code, message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_match_the_engine() {
        let all = [UpdatesError::Off, UpdatesError::Storage("x".into())];
        assert_eq!(all.len(), UpdatesCode::ALL.len());
        for e in all {
            let engine = e.code();
            assert_eq!(AppError::from(e).code_str(), engine);
        }
    }
}
