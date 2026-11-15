//! API keys in the login Keychain: generic passwords with service
//! [`KEYCHAIN_SERVICE`] and account [`AiProvider::id`].

use lumora_core::ai::{AiError, AiProvider, KeyStore, KEYCHAIN_SERVICE};
use security_framework::passwords::{delete_generic_password, get_generic_password, set_generic_password};

/// `errSecItemNotFound`.
const ERR_SEC_ITEM_NOT_FOUND: i32 = -25300;

/// The macOS Keychain as a [`KeyStore`].
#[derive(Debug, Default, Clone, Copy)]
pub struct Keychain;

fn err(e: security_framework::base::Error) -> AiError {
    AiError::Keychain(e.message().unwrap_or_else(|| format!("OSStatus {}", e.code())))
}

impl KeyStore for Keychain {
    fn get(&self, provider: AiProvider) -> Result<Option<String>, AiError> {
        match get_generic_password(KEYCHAIN_SERVICE, provider.id()) {
            Ok(bytes) => Ok(Some(String::from_utf8_lossy(&bytes).into_owned()).filter(|k| !k.is_empty())),
            Err(e) if e.code() == ERR_SEC_ITEM_NOT_FOUND => Ok(None),
            Err(e) => Err(err(e)),
        }
    }

    fn set(&self, provider: AiProvider, key: &str) -> Result<(), AiError> {
        let key = key.trim();
        if key.is_empty() {
            return self.delete(provider);
        }
        set_generic_password(KEYCHAIN_SERVICE, provider.id(), key.as_bytes()).map_err(err)
    }

    fn delete(&self, provider: AiProvider) -> Result<(), AiError> {
        match delete_generic_password(KEYCHAIN_SERVICE, provider.id()) {
            Ok(()) => Ok(()),
            Err(e) if e.code() == ERR_SEC_ITEM_NOT_FOUND => Ok(()),
            Err(e) => Err(err(e)),
        }
    }
}
