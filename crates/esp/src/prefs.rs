//! The Settings dialog: start-at-login, and export/import of the Oortery setup.

use crate::{backups, syncs, Ui};
use adw::prelude::*;
use gtk::gio;
use std::rc::Rc;

/// The command an autostart entry should run: the AppImage path if we are one, else this binary.
fn autostart_exec() -> String {
    std::env::var("APPIMAGE")
        .ok()
        .or_else(|| {
            std::env::current_exe()
                .ok()
                .map(|p| p.to_string_lossy().into_owned())
        })
        .unwrap_or_default()
}

/// Present the Settings dialog over the main window.
pub fn present(ui: &Rc<Ui>) {
    let dialog = adw::PreferencesDialog::new();
    dialog.set_title("Settings");
    let page = adw::PreferencesPage::new();

    // Startup: launch at login so scheduled work keeps running.
    let g_start = adw::PreferencesGroup::builder().title("Startup").build();
    let autostart = adw::SwitchRow::builder()
        .title("Start Oortery at login")
        .subtitle("Launch automatically so scheduled syncs and backups keep running.")
        .build();
    autostart.set_active(esp_core::autostart_enabled());
    let ui_a = ui.clone();
    autostart.connect_active_notify(move |sw| {
        if let Err(e) = esp_core::set_autostart(sw.is_active(), &autostart_exec()) {
            ui_a.toast(&format!("Could not change startup: {e}"));
        }
    });
    g_start.add(&autostart);
    page.add(&g_start);

    // Settings file: move the setup to another computer.
    let g_file = adw::PreferencesGroup::builder()
        .title("Settings file")
        .description(
            "Move your syncs, backups and schedules to another computer. No passwords or tokens \
             are exported, so you reconnect remotes (sign in again) on the other side.",
        )
        .build();

    let export_row = adw::ActionRow::builder().title("Export settings").build();
    let export_btn = gtk::Button::with_label("Export");
    export_btn.set_valign(gtk::Align::Center);
    export_row.add_suffix(&export_btn);
    g_file.add(&export_row);

    let import_row = adw::ActionRow::builder().title("Import settings").build();
    let import_btn = gtk::Button::with_label("Import");
    import_btn.set_valign(gtk::Align::Center);
    import_row.add_suffix(&import_btn);
    g_file.add(&import_row);

    page.add(&g_file);

    // Security: an optional password to open the app.
    let g_sec = adw::PreferencesGroup::builder()
        .title("Security")
        .description(
            "A password to open Oortery on a shared machine. It gates access to the app; it does \
             not encrypt your remotes or backups. If you forget it, delete lock.hash in the config \
             folder to reset.",
        )
        .build();
    if esp_core::lock_enabled() {
        let change_row = adw::ActionRow::builder().title("App lock is on").build();
        change_row.add_prefix(&gtk::Image::from_icon_name("changes-prevent-symbolic"));
        let change = gtk::Button::with_label("Change");
        change.set_valign(gtk::Align::Center);
        let ui_c = ui.clone();
        change.connect_clicked(move |_| set_password_dialog(&ui_c));
        change_row.add_suffix(&change);
        g_sec.add(&change_row);

        let remove_row = adw::ActionRow::builder().title("Remove app lock").build();
        let remove = gtk::Button::with_label("Remove");
        remove.add_css_class("destructive-action");
        remove.set_valign(gtk::Align::Center);
        let ui_r = ui.clone();
        remove.connect_clicked(move |_| remove_lock_dialog(&ui_r));
        remove_row.add_suffix(&remove);
        g_sec.add(&remove_row);
    } else {
        let set_row = adw::ActionRow::builder()
            .title("App lock is off")
            .subtitle("Require a password to open Oortery.")
            .build();
        set_row.add_prefix(&gtk::Image::from_icon_name("changes-allow-symbolic"));
        let setb = gtk::Button::with_label("Set a password");
        setb.set_valign(gtk::Align::Center);
        let ui_s = ui.clone();
        setb.connect_clicked(move |_| set_password_dialog(&ui_s));
        set_row.add_suffix(&setb);
        g_sec.add(&set_row);
    }
    page.add(&g_sec);

    dialog.add(&page);

    let ui_e = ui.clone();
    export_btn.connect_clicked(move |_| export_settings(&ui_e));
    let ui_i = ui.clone();
    import_btn.connect_clicked(move |_| import_settings(&ui_i));

    dialog.present(Some(&ui.window));
}

fn export_settings(ui: &Rc<Ui>) {
    let chooser = gtk::FileDialog::builder()
        .title("Export Oortery settings")
        .initial_name("oortery-settings.json")
        .build();
    let ui = ui.clone();
    let win = ui.window.clone();
    chooser.save(Some(&win), gio::Cancellable::NONE, move |res| {
        let Ok(file) = res else { return };
        let Some(path) = file.path() else { return };
        match esp_core::export_bundle() {
            Ok(json) => match std::fs::write(&path, json) {
                Ok(()) => ui.toast("Settings exported"),
                Err(e) => ui.toast(&format!("Could not write the file: {e}")),
            },
            Err(e) => ui.toast(&format!("Could not export: {e}")),
        }
    });
}

fn import_settings(ui: &Rc<Ui>) {
    let chooser = gtk::FileDialog::builder()
        .title("Import Oortery settings")
        .build();
    let ui = ui.clone();
    let win = ui.window.clone();
    chooser.open(Some(&win), gio::Cancellable::NONE, move |res| {
        let Ok(file) = res else { return };
        let Some(path) = file.path() else { return };
        let outcome = std::fs::read_to_string(&path)
            .map_err(|e| e.to_string())
            .and_then(|s| esp_core::import_bundle(&s).map_err(|e| e.to_string()));
        match outcome {
            Ok(result) => {
                crate::refresh(&ui);
                syncs::refresh(&ui);
                backups::refresh(&ui);
                if result.missing_remotes.is_empty() {
                    ui.toast(&format!(
                        "Imported {} syncs and {} backups",
                        result.jobs, result.backups
                    ));
                } else {
                    reconnect_notice(&ui, &result);
                }
            }
            Err(e) => ui.toast(&format!("Could not import: {e}")),
        }
    });
}

/// Prompt for a new app-lock password (entered twice), then set it.
fn set_password_dialog(ui: &Rc<Ui>) {
    let new = adw::PasswordEntryRow::builder()
        .title("New password")
        .build();
    let confirm = adw::PasswordEntryRow::builder()
        .title("Confirm password")
        .build();
    let list = gtk::ListBox::new();
    list.add_css_class("boxed-list");
    list.set_selection_mode(gtk::SelectionMode::None);
    list.append(&new);
    list.append(&confirm);

    let dialog = adw::AlertDialog::new(Some("Set a password"), None);
    dialog.set_extra_child(Some(&list));
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("set", "Set");
    dialog.set_response_appearance("set", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("set"));
    dialog.set_close_response("cancel");

    let win = ui.window.clone();
    let ui = ui.clone();
    dialog.connect_response(None, move |_, resp| {
        if resp != "set" {
            return;
        }
        let (a, b) = (new.text().to_string(), confirm.text().to_string());
        if a.is_empty() {
            ui.toast("Enter a password.");
        } else if a != b {
            ui.toast("The passwords do not match.");
        } else {
            match esp_core::set_lock(&a) {
                Ok(()) => ui.toast("App lock enabled. It takes effect next time you open Oortery."),
                Err(e) => ui.toast(&format!("Could not set the password: {e}")),
            }
        }
    });
    dialog.present(Some(&win));
}

/// Confirm the current password, then remove the app lock.
fn remove_lock_dialog(ui: &Rc<Ui>) {
    let current = adw::PasswordEntryRow::builder()
        .title("Current password")
        .build();
    let list = gtk::ListBox::new();
    list.add_css_class("boxed-list");
    list.set_selection_mode(gtk::SelectionMode::None);
    list.append(&current);

    let dialog = adw::AlertDialog::new(
        Some("Remove app lock"),
        Some("Enter your current password to turn the lock off."),
    );
    dialog.set_extra_child(Some(&list));
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("remove", "Remove");
    dialog.set_response_appearance("remove", adw::ResponseAppearance::Destructive);
    dialog.set_close_response("cancel");

    let win = ui.window.clone();
    let ui = ui.clone();
    dialog.connect_response(None, move |_, resp| {
        if resp != "remove" {
            return;
        }
        if esp_core::verify_lock(&current.text()) {
            match esp_core::clear_lock() {
                Ok(()) => ui.toast("App lock removed."),
                Err(e) => ui.toast(&format!("Could not remove the lock: {e}")),
            }
        } else {
            ui.toast("Incorrect password.");
        }
    });
    dialog.present(Some(&win));
}

/// After an import, tell the user which remotes they still need to reconnect here.
fn reconnect_notice(ui: &Rc<Ui>, result: &esp_core::ImportResult) {
    let body = format!(
        "Imported {} syncs and {} backups.\n\nReconnect these remotes to finish (sign in again): {}",
        result.jobs,
        result.backups,
        result.missing_remotes.join(", ")
    );
    let dialog = adw::AlertDialog::new(Some("Almost there"), Some(&body));
    dialog.add_response("ok", "Got it");
    dialog.set_default_response(Some("ok"));
    dialog.present(Some(&ui.window));
}
