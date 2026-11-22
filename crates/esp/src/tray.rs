//! A cross-desktop system-tray icon (StatusNotifierItem via ksni, pure Rust over D-Bus), so the
//! app is reachable while it runs in the background. Open re-shows the window; Quit really exits.
//!
//! KDE Plasma, XFCE and most panels show this natively; GNOME needs the AppIndicator/
//! KStatusNotifierItem Shell extension. When there is no tray host, this degrades to nothing: the
//! window is still reachable by launching Oortery again, so we never strand the user.

use crate::Ui;
use adw::prelude::*;
use gtk::glib;
use std::rc::Rc;

/// What a tray menu click asks the UI thread to do.
enum TrayMsg {
    Open,
    Quit,
}

struct OorteryTray {
    tx: async_channel::Sender<TrayMsg>,
}

impl ksni::Tray for OorteryTray {
    fn id(&self) -> String {
        crate::APP_ID.to_string()
    }
    fn title(&self) -> String {
        "Oortery".to_string()
    }
    fn icon_name(&self) -> String {
        crate::APP_ID.to_string()
    }
    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::StandardItem;
        vec![
            StandardItem {
                label: "Open Oortery".into(),
                activate: Box::new(|t: &mut Self| {
                    let _ = t.tx.send_blocking(TrayMsg::Open);
                }),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "Quit Oortery".into(),
                activate: Box::new(|t: &mut Self| {
                    let _ = t.tx.send_blocking(TrayMsg::Quit);
                }),
                ..Default::default()
            }
            .into(),
        ]
    }
}

/// Install the tray icon and route its clicks onto the GTK main loop. Best-effort: if no tray host
/// is present the spawn fails and we simply carry on (the window stays reachable by relaunch).
pub fn setup(ui: &Rc<Ui>) {
    use ksni::blocking::TrayMethods;
    let (tx, rx) = async_channel::unbounded();
    let tray = OorteryTray { tx };
    let handle = match tray.spawn() {
        Ok(h) => h,
        Err(_) => return, // no StatusNotifier host (e.g. GNOME without the extension): degrade quietly
    };
    // Keep the tray service alive for the whole process.
    std::mem::forget(handle);

    let ui = ui.clone();
    glib::spawn_future_local(async move {
        while let Ok(msg) = rx.recv().await {
            match msg {
                TrayMsg::Open => {
                    ui.window.set_visible(true);
                    ui.window.present();
                }
                TrayMsg::Quit => {
                    esp_core::desktop::prune_places();
                    if let Some(app) = ui.window.application() {
                        app.quit();
                    }
                }
            }
        }
    });
}
