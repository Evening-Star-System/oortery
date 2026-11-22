//! The Mounts page: make a connected cloud appear as a folder on this computer. Each remote
//! gets a Mount / Unmount toggle driven by the shared rcd daemon (the same one that runs
//! syncs). Mounts live for the app session; the default mountpoint is `$HOME/Oortery/<remote>`.

use crate::Ui;
use adw::prelude::*;
use gtk::glib;
use std::path::PathBuf;
use std::rc::Rc;

/// Build the Mounts page. Returns the page and the list to refresh.
pub fn page() -> (gtk::Widget, gtk::ListBox) {
    let clamp = adw::Clamp::new();
    clamp.set_maximum_size(820);
    let scroll = gtk::ScrolledWindow::new();
    scroll.set_vexpand(true);
    let col = gtk::Box::new(gtk::Orientation::Vertical, 10);
    col.set_margin_top(24);
    col.set_margin_bottom(24);
    col.set_margin_start(24);
    col.set_margin_end(24);

    let heading = gtk::Label::new(Some("Mounts"));
    heading.add_css_class("title-4");
    heading.set_xalign(0.0);
    let sub = gtk::Label::new(Some(
        "Open a cloud as a folder on this computer. No copying, no waiting.",
    ));
    sub.add_css_class("dim");
    sub.set_xalign(0.0);
    sub.set_margin_bottom(8);

    let list = gtk::ListBox::new();
    list.add_css_class("boxed-list");
    list.set_selection_mode(gtk::SelectionMode::None);

    col.append(&heading);
    col.append(&sub);
    col.append(&list);
    scroll.set_child(Some(&col));
    clamp.set_child(Some(&scroll));
    (clamp.upcast(), list)
}

/// The default mountpoint for a remote: `$HOME/Oortery/<remote>`.
fn point_for(name: &str) -> String {
    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/tmp".into()))
        .join("Oortery")
        .join(name)
        .to_string_lossy()
        .into_owned()
}

/// Rebuild the list: a row per remote with its current mount state and a toggle.
pub fn refresh(ui: &Rc<Ui>) {
    while let Some(child) = ui.mounts_list.first_child() {
        ui.mounts_list.remove(&child);
    }
    let remotes = esp_core::list_remotes().unwrap_or_default();
    if remotes.is_empty() {
        let row = adw::ActionRow::builder()
            .title("No remotes to mount")
            .subtitle("Connect a cloud first, then mount it here.")
            .build();
        row.add_prefix(&gtk::Image::from_icon_name("folder-symbolic"));
        ui.mounts_list.append(&row);
        return;
    }

    // Which of our default mountpoints are live right now (daemon may not be running yet).
    let mounted: Vec<String> = ui
        .rcd
        .borrow()
        .as_ref()
        .and_then(|rcd| esp_core::list_mounts(rcd.port()).ok())
        .map(|ms| ms.into_iter().map(|m| m.mount_point).collect())
        .unwrap_or_default();

    for r in &remotes {
        let point = point_for(&r.name);
        let is_mounted = mounted.contains(&point);

        let row = adw::ActionRow::builder()
            .title(&r.name)
            .subtitle(if is_mounted {
                point.clone()
            } else {
                esp_core::backend_label(&r.backend).to_string()
            })
            .build();
        row.add_prefix(&gtk::Image::from_icon_name("folder-symbolic"));

        let btn = gtk::Button::with_label(if is_mounted { "Unmount" } else { "Mount" });
        btn.set_valign(gtk::Align::Center);
        if is_mounted {
            btn.add_css_class("destructive-action");
        } else {
            btn.add_css_class("suggested-action");
        }
        let ui2 = ui.clone();
        let name = r.name.clone();
        btn.connect_clicked(move |_| {
            if is_mounted {
                unmount(&ui2, &name);
            } else {
                mount(&ui2, &name);
            }
        });
        row.add_suffix(&btn);
        ui.mounts_list.append(&row);
    }

    // Headless aid: click the first Mount/Unmount toggle once the page maps.
    if crate::aid("ESP_MOUNT_AUTORUN").is_some() {
        if let Some(first) = ui.mounts_list.first_child() {
            if let Some(btn) = first_button(&first) {
                glib::timeout_add_local_once(std::time::Duration::from_millis(800), move || {
                    btn.activate();
                });
            }
        }
    }
}

/// The first button found within a row (the Mount/Unmount toggle; used by the headless aid).
fn first_button(root: &gtk::Widget) -> Option<gtk::Button> {
    let mut stack = vec![root.clone()];
    while let Some(w) = stack.pop() {
        if let Ok(btn) = w.clone().downcast::<gtk::Button>() {
            return Some(btn);
        }
        let mut child = w.first_child();
        while let Some(c) = child {
            stack.push(c.clone());
            child = c.next_sibling();
        }
    }
    None
}

fn mount(ui: &Rc<Ui>, name: &str) {
    let rcd = match ui.ensure_rcd() {
        Ok(r) => r,
        Err(e) => {
            ui.toast(&format!("Could not start the engine: {e}"));
            return;
        }
    };
    let point = point_for(name);
    if let Err(e) = std::fs::create_dir_all(&point) {
        ui.toast(&format!("Could not create {point}: {e}"));
        return;
    }
    // Mount on a worker thread: a cloud (e.g. Google Drive) can take seconds to initialise its
    // VFS, and doing that on the UI thread freezes the whole app.
    ui.toast(&format!("Mounting {name}\u{2026}"));
    let (tx, rx) = async_channel::bounded(1);
    let (port, fs, point_t) = (rcd.port(), format!("{name}:"), point.clone());
    std::thread::spawn(move || {
        let _ = tx.send_blocking(esp_core::mount(port, &fs, &point_t).map_err(|e| e.to_string()));
    });
    let (ui_c, name_c, point_c) = (ui.clone(), name.to_string(), point);
    glib::spawn_future_local(async move {
        match rx.recv().await {
            Ok(Ok(())) => {
                // Let the daemon register the mount before we re-query listmounts, so the row
                // flips to "Unmount" on the first refresh.
                glib::timeout_future(std::time::Duration::from_millis(500)).await;
                refresh(&ui_c);
                ui_c.toast(&format!("Mounted {name_c} at {point_c}"));
            }
            Ok(Err(e)) => {
                refresh(&ui_c);
                ui_c.toast(&format!("Could not mount {name_c}: {e}"));
            }
            Err(_) => {}
        }
    });
}

/// Confirm removing a remote, then unmount it first if it is mounted (so the OS file manager updates
/// and no stale mount lingers), delete its connection config, and refresh every page. Removing a
/// remote never deletes the files in the cloud, only the connection here.
pub(crate) fn confirm_remove_remote(ui: &Rc<Ui>, name: &str) {
    let dialog = adw::AlertDialog::new(
        Some(&format!("Remove {name}?")),
        Some(
            "This removes the connection, and unmounts it first if it is mounted. It does not delete \
             any of your files in the cloud.",
        ),
    );
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("remove", "Remove");
    dialog.set_response_appearance("remove", adw::ResponseAppearance::Destructive);
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");
    let (ui2, name2) = (ui.clone(), name.to_string());
    dialog.connect_response(None, move |_, resp| {
        if resp == "remove" {
            remove_now(&ui2, &name2);
        }
    });
    dialog.present(Some(&ui.window));
}

/// Unmount the remote if it is mounted, then delete its config and refresh everything.
fn remove_now(ui: &Rc<Ui>, name: &str) {
    let point = point_for(name);
    let mounted = ui
        .rcd
        .borrow()
        .as_ref()
        .and_then(|rcd| esp_core::list_mounts(rcd.port()).ok())
        .map(|ms| ms.into_iter().any(|m| m.mount_point == point))
        .unwrap_or(false);

    if mounted {
        if let Some(rcd) = ui.rcd.borrow().as_ref().cloned() {
            ui.toast(&format!("Unmounting {name}\u{2026}"));
            let (tx, rx) = async_channel::bounded(1);
            let (port, point_t) = (rcd.port(), point);
            std::thread::spawn(move || {
                let _ =
                    tx.send_blocking(esp_core::unmount(port, &point_t).map_err(|e| e.to_string()));
            });
            let (ui_c, name_c) = (ui.clone(), name.to_string());
            glib::spawn_future_local(async move {
                let _ = rx.recv().await; // proceed whether or not the unmount reported success
                finish_remove(&ui_c, &name_c);
            });
            return;
        }
    }
    finish_remove(ui, name);
}

fn finish_remove(ui: &Rc<Ui>, name: &str) {
    match esp_core::delete_remote(name) {
        Ok(()) => {
            crate::refresh_all(ui);
            ui.toast(&format!("Removed {name}"));
        }
        Err(e) => ui.error(&format!("Could not remove {name}: {e}")),
    }
}

fn unmount(ui: &Rc<Ui>, name: &str) {
    let Some(rcd) = ui.rcd.borrow().as_ref().cloned() else {
        refresh(ui);
        return;
    };
    // Unmount off the UI thread too, for the same reason (a busy mount can be slow to release).
    ui.toast(&format!("Unmounting {name}\u{2026}"));
    let (tx, rx) = async_channel::bounded(1);
    let (port, point_t) = (rcd.port(), point_for(name));
    std::thread::spawn(move || {
        let _ = tx.send_blocking(esp_core::unmount(port, &point_t).map_err(|e| e.to_string()));
    });
    let (ui_c, name_c) = (ui.clone(), name.to_string());
    glib::spawn_future_local(async move {
        match rx.recv().await {
            Ok(Ok(())) => {
                refresh(&ui_c);
                ui_c.toast(&format!("Unmounted {name_c}"));
            }
            Ok(Err(e)) => {
                refresh(&ui_c);
                ui_c.toast(&format!("Could not unmount {name_c}: {e}"));
            }
            Err(_) => {}
        }
    });
}
