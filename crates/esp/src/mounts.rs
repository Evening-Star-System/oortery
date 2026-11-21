//! The Mounts page: make a connected cloud appear as a folder on this computer. Each remote
//! gets a Mount / Unmount toggle driven by the shared rcd daemon (the same one that runs
//! syncs). Mounts live for the app session; the default mountpoint is `$HOME/Oortery/<remote>`.

use crate::Ui;
use adw::prelude::*;
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
    match esp_core::mount(rcd.port(), &format!("{name}:"), &point) {
        Ok(()) => {
            refresh(ui);
            ui.toast(&format!("Mounted {name} at {point}"));
        }
        Err(e) => ui.toast(&format!("Could not mount {name}: {e}")),
    }
}

fn unmount(ui: &Rc<Ui>, name: &str) {
    let Some(rcd) = ui.rcd.borrow().as_ref().cloned() else {
        refresh(ui);
        return;
    };
    let point = point_for(name);
    match esp_core::unmount(rcd.port(), &point) {
        Ok(()) => {
            refresh(ui);
            ui.toast(&format!("Unmounted {name}"));
        }
        Err(e) => ui.toast(&format!("Could not unmount {name}: {e}")),
    }
}
