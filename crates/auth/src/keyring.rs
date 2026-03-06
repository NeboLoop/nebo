use tracing::{debug, warn};

const SERVICE_NAME: &str = "nebo";
const ACCOUNT_NAME: &str = "master-encryption-key";

/// Check if the system keyring is available.
pub fn available() -> bool {
    match keyring::Entry::new(SERVICE_NAME, ACCOUNT_NAME) {
        Ok(entry) => {
            // Try to get — if it fails with NoEntry that's fine, means keyring works
            match entry.get_password() {
                Ok(_) => true,
                Err(keyring::Error::NoEntry) => true,
                Err(keyring::Error::PlatformFailure(_)) => false,
                Err(keyring::Error::NoStorageAccess(_)) => false,
                Err(_) => false,
            }
        }
        Err(_) => false,
    }
}

/// Get the master encryption key from the OS keychain.
pub fn get() -> Option<String> {
    match keyring::Entry::new(SERVICE_NAME, ACCOUNT_NAME) {
        Ok(entry) => match entry.get_password() {
            Ok(key) => {
                debug!("retrieved master key from keyring");
                Some(key)
            }
            Err(keyring::Error::NoEntry) => {
                debug!("no master key in keyring");
                None
            }
            Err(e) => {
                warn!(error = %e, "failed to read from keyring");
                None
            }
        },
        Err(e) => {
            warn!(error = %e, "failed to create keyring entry");
            None
        }
    }
}

/// Store the master encryption key in the OS keychain.
pub fn set(key: &str) -> Result<(), String> {
    let entry = keyring::Entry::new(SERVICE_NAME, ACCOUNT_NAME)
        .map_err(|e| format!("keyring entry error: {}", e))?;
    entry
        .set_password(key)
        .map_err(|e| format!("keyring set error: {}", e))?;
    debug!("stored master key in keyring");
    Ok(())
}

/// Delete the master encryption key from the OS keychain.
pub fn delete() -> Result<(), String> {
    let entry = keyring::Entry::new(SERVICE_NAME, ACCOUNT_NAME)
        .map_err(|e| format!("keyring entry error: {}", e))?;
    match entry.delete_credential() {
        Ok(()) => {
            debug!("deleted master key from keyring");
            Ok(())
        }
        Err(keyring::Error::NoEntry) => Ok(()), // already gone
        Err(e) => Err(format!("keyring delete error: {}", e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_keyring_available() {
        // Just verify the function doesn't panic
        let _avail = available();
    }

    #[test]
    fn test_keyring_get_nonexistent() {
        // On CI or headless systems, keyring may not be available.
        // This test verifies it doesn't panic.
        let _result = get();
    }
}
