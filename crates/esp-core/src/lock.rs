//! Optional app lock: a password to open Oortery. The password is never stored; only an
//! Argon2id hash (with a random salt, PHC-encoded) lives in a 0600 file under the config dir.
//! This guards the control surface and the view of your data on a shared machine. It does not
//! encrypt the rclone config or restic repos (those have their own secrets); it is an access
//! gate, and losing the password means deleting `lock.hash` by hand to reset it.

use crate::jobs::config_dir;
use anyhow::{anyhow, Context, Result};
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use std::path::PathBuf;

fn lock_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("lock.hash"))
}

/// True if an app-lock password is set.
pub fn lock_enabled() -> bool {
    lock_path().map(|p| p.exists()).unwrap_or(false)
}

/// Set (or change) the app-lock password: hash it with Argon2id and store the hash 0600.
pub fn set_lock(password: &str) -> Result<()> {
    if password.is_empty() {
        return Err(anyhow!("the password cannot be empty"));
    }
    let mut salt_bytes = [0u8; 16];
    getrandom::getrandom(&mut salt_bytes).map_err(|e| anyhow!("could not generate a salt: {e}"))?;
    let salt = SaltString::encode_b64(&salt_bytes).map_err(|e| anyhow!("salt encoding: {e}"))?;
    let hash = Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map_err(|e| anyhow!("hashing failed: {e}"))?
        .to_string();

    let path = lock_path()?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&path, hash).with_context(|| format!("writing {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

/// Remove the app lock.
pub fn clear_lock() -> Result<()> {
    let path = lock_path()?;
    if path.exists() {
        std::fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))?;
    }
    Ok(())
}

/// Check a password against the stored hash. False on any error (no lock, unreadable, mismatch).
pub fn verify(password: &str) -> bool {
    let Ok(path) = lock_path() else {
        return false;
    };
    let Ok(stored) = std::fs::read_to_string(&path) else {
        return false;
    };
    let Ok(parsed) = PasswordHash::new(stored.trim()) else {
        return false;
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_verify_clear() {
        let _env = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("oortery-lock-{}", std::process::id()));
        std::env::set_var("XDG_CONFIG_HOME", &dir);
        let _ = std::fs::remove_dir_all(&dir);

        assert!(!lock_enabled());
        set_lock("correct horse battery staple").unwrap();
        assert!(lock_enabled());
        assert!(verify("correct horse battery staple"));
        assert!(!verify("wrong password"));
        // The plaintext is never in the file.
        let stored = std::fs::read_to_string(dir.join("oortery").join("lock.hash")).unwrap();
        assert!(!stored.contains("correct horse"));
        assert!(stored.starts_with("$argon2"));

        clear_lock().unwrap();
        assert!(!lock_enabled());
        assert!(!verify("correct horse battery staple"));

        std::env::remove_var("XDG_CONFIG_HOME");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn empty_password_rejected() {
        let _env = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("oortery-lock2-{}", std::process::id()));
        std::env::set_var("XDG_CONFIG_HOME", &dir);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(set_lock("").is_err());
        std::env::remove_var("XDG_CONFIG_HOME");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
