//! Launch-at-login, the portable way: an XDG autostart entry in `~/.config/autostart`. The
//! freedesktop Autostart spec is honored by all the main Linux desktops (GNOME, KDE Plasma,
//! XFCE, Cinnamon, MATE, Budgie, LXQt), so one `.desktop` file covers them. (Flatpak sandboxes
//! use the Background portal instead; that is a later addition.)

use anyhow::{Context, Result};
use std::path::PathBuf;

const ENTRY: &str = "app.eveningstar.Oortery.desktop";

fn autostart_file() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .map(|base| base.join("autostart").join(ENTRY))
}

/// True if Oortery is set to start at login.
pub fn autostart_enabled() -> bool {
    autostart_file().map(|p| p.exists()).unwrap_or(false)
}

/// Turn launch-at-login on or off. `exec` is the command the entry runs: for an AppImage pass
/// its `$APPIMAGE` path (stable across runs), otherwise the installed binary path.
pub fn set_autostart(enabled: bool, exec: &str) -> Result<()> {
    let file =
        autostart_file().context("no HOME or XDG_CONFIG_HOME to place the autostart entry")?;
    if enabled {
        if let Some(dir) = file.parent() {
            std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        let contents = format!(
            "[Desktop Entry]\n\
             Type=Application\n\
             Name=Oortery\n\
             Comment=Mission control for all your data\n\
             Exec={exec}\n\
             Icon=app.eveningstar.Oortery\n\
             Terminal=false\n\
             X-GNOME-Autostart-enabled=true\n"
        );
        std::fs::write(&file, contents).with_context(|| format!("writing {}", file.display()))?;
    } else if file.exists() {
        std::fs::remove_file(&file).with_context(|| format!("removing {}", file.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toggle_autostart_writes_and_removes_entry() {
        let _env = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("oortery-autostart-{}", std::process::id()));
        std::env::set_var("XDG_CONFIG_HOME", &dir);
        let _ = std::fs::remove_dir_all(&dir);

        assert!(!autostart_enabled());
        set_autostart(true, "/opt/Oortery.AppImage").unwrap();
        assert!(autostart_enabled());
        let body = std::fs::read_to_string(dir.join("autostart").join(ENTRY)).unwrap();
        assert!(body.contains("Exec=/opt/Oortery.AppImage"));
        set_autostart(false, "/opt/Oortery.AppImage").unwrap();
        assert!(!autostart_enabled());

        std::env::remove_var("XDG_CONFIG_HOME");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
