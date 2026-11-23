//! Credential storage in the OS keyring, with a graceful fallback.
//!
//! Backup repository passwords are the one secret Oortery must keep: lose it and the snapshots are
//! unreadable forever. We store it in the OS keyring (the freedesktop Secret Service on Linux via a
//! pure-Rust backend; Keychain on macOS and Credential Manager on Windows once those land) so it is
//! not a plaintext file on disk. Where no keyring is available (a headless box, a minimal distro
//! with no secret-service provider), the caller falls back to a 0600 file so backups still work.
//!
//! Keyed by a stable service id plus the backup job id.

const SERVICE: &str = "app.eveningstar.Oortery";

fn entry(id: &str) -> Option<keyring::Entry> {
    keyring::Entry::new(SERVICE, id).ok()
}

/// Store `password` for `id` in the OS keyring. Returns true if the keyring accepted it; false when
/// no keyring is available (the caller should then fall back to a file).
pub fn keyring_set(id: &str, password: &str) -> bool {
    entry(id)
        .map(|e| e.set_password(password).is_ok())
        .unwrap_or(false)
}

/// The password for `id` from the keyring, if present and a keyring is available.
pub fn keyring_get(id: &str) -> Option<String> {
    entry(id)?.get_password().ok()
}

/// Remove `id`'s password from the keyring (best effort; absent or no-keyring is fine).
pub fn keyring_delete(id: &str) {
    if let Some(e) = entry(id) {
        let _ = e.delete_credential();
    }
}

/// Whether a working keyring is reachable right now. Probes with a round-trip on a sentinel so a
/// missing secret-service provider is detected, not assumed. Cheap; safe to call before migrating.
pub fn keyring_available() -> bool {
    // set+delete a sentinel: this exercises the real backend the way keyring_set does, so it only
    // reports available when a write would actually succeed.
    let probe = "__oortery_probe__";
    if keyring_set(probe, "x") {
        keyring_delete(probe);
        true
    } else {
        false
    }
}
