//! Profile keys in the system keychain: service `dramatis`, account `profile.<name>`.
//! A key is never read from the environment or a file, and never printed.

use crate::error::{Error, Result};
use crate::types::Secret;

pub const SERVICE: &str = "dramatis";

pub fn account(profile: &str) -> String {
    format!("profile.{profile}")
}

fn entry(profile: &str) -> Result<keyring::Entry> {
    keyring::Entry::new(SERVICE, &account(profile)).map_err(|e| Error::Keychain(e.to_string()))
}

/// `None` when the profile has no stored key.
pub fn get(profile: &str) -> Result<Option<Secret>> {
    match entry(profile)?.get_password() {
        Ok(k) => Ok(Some(Secret::new(k))),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(Error::Keychain(e.to_string())),
    }
}

pub fn set(profile: &str, key: &str) -> Result<()> {
    entry(profile)?
        .set_password(key)
        .map_err(|e| Error::Keychain(e.to_string()))
}

pub fn delete(profile: &str) -> Result<()> {
    match entry(profile)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(Error::Keychain(e.to_string())),
    }
}
