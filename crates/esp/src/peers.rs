//! Peers: account-free, cloud-free sync straight to your other devices and your phone, over the
//! bundled Syncthing engine. Show this device's code (and a QR to scan from a phone), pair another
//! device by its code, and share a folder with it. The engine starts lazily the first time this
//! page is opened, so Oortery does not run a P2P daemon unless you actually use it.

use crate::Ui;
use adw::prelude::*;
use gtk::glib;
use std::rc::Rc;

/// Handles build_ui keeps so the page can be filled in once the daemon is up.
pub struct PeersPage {
    pub widget: gtk::Widget,
    pub list: gtk::ListBox,
    pub id_label: gtk::Label,
    pub qr: gtk::DrawingArea,
    pub add_btn: gtk::Button,
}

/// Build the (initially empty) Peers page. `refresh` fills it once the engine has started.
pub fn page() -> PeersPage {
    let clamp = adw::Clamp::new();
    clamp.set_maximum_size(820);
    let scroll = gtk::ScrolledWindow::new();
    scroll.set_vexpand(true);
    let col = gtk::Box::new(gtk::Orientation::Vertical, 14);
    col.set_margin_top(24);
    col.set_margin_bottom(24);
    col.set_margin_start(24);
    col.set_margin_end(24);

    let heading = gtk::Label::new(Some("Peers"));
    heading.add_css_class("title-4");
    heading.set_xalign(0.0);
    let sub = gtk::Label::new(Some(
        "Sync folders straight to your other devices and your phone. No cloud, no account.",
    ));
    sub.add_css_class("dim");
    sub.set_xalign(0.0);
    sub.set_wrap(true);
    col.append(&heading);
    col.append(&sub);

    // This device: a QR to scan from a phone, plus the code to copy.
    let me_group = adw::PreferencesGroup::builder().title("This device").build();
    let qr = gtk::DrawingArea::new();
    qr.set_content_width(200);
    qr.set_content_height(200);
    qr.set_halign(gtk::Align::Center);
    qr.set_margin_top(6);
    qr.set_margin_bottom(6);
    let qr_caption = gtk::Label::new(Some(
        "Scan with Syncthing on your phone, or share the code below.",
    ));
    qr_caption.add_css_class("caption");
    qr_caption.add_css_class("dim");
    qr_caption.set_wrap(true);
    qr_caption.set_justify(gtk::Justification::Center);

    let id_label = gtk::Label::new(Some("Starting the P2P engine\u{2026}"));
    id_label.add_css_class("monospace");
    id_label.set_wrap(true);
    id_label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    id_label.set_selectable(true);
    id_label.set_xalign(0.0);
    let copy_btn = gtk::Button::from_icon_name("edit-copy-symbolic");
    copy_btn.set_tooltip_text(Some("Copy this device's code"));
    copy_btn.set_valign(gtk::Align::Center);
    copy_btn.add_css_class("flat");
    let id_row = adw::ActionRow::builder().title("Your device code").build();
    id_row.add_suffix(&copy_btn);

    let me_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
    me_box.append(&qr);
    me_box.append(&qr_caption);
    me_group.add(&me_box);
    me_group.add(&id_row);
    // The selectable code sits under the row so it can wrap full-width.
    let id_wrap = gtk::Box::new(gtk::Orientation::Vertical, 0);
    id_wrap.set_margin_start(6);
    id_wrap.set_margin_end(6);
    id_wrap.set_margin_bottom(6);
    id_wrap.append(&id_label);
    me_group.add(&id_wrap);
    col.append(&me_group);

    // Paired devices, with an Add button in the group header.
    let add_btn = gtk::Button::with_label("Add a device");
    add_btn.add_css_class("suggested-action");
    add_btn.set_valign(gtk::Align::Center);
    let peers_group = adw::PreferencesGroup::builder()
        .title("Paired devices")
        .build();
    peers_group.set_header_suffix(Some(&add_btn));
    let list = gtk::ListBox::new();
    list.add_css_class("boxed-list");
    list.set_selection_mode(gtk::SelectionMode::None);
    peers_group.add(&list);
    col.append(&peers_group);

    // Copy the device code to the clipboard.
    let id_for_copy = id_label.clone();
    copy_btn.connect_clicked(move |btn| {
        let text = id_for_copy.text();
        if !text.is_empty() {
            btn.display().clipboard().set_text(&text);
        }
    });

    scroll.set_child(Some(&col));
    clamp.set_child(Some(&scroll));
    PeersPage {
        widget: clamp.upcast(),
        list,
        id_label,
        qr,
        add_btn,
    }
}

/// Start the engine (first visit only) off the UI thread, then fill in the device code, QR and the
/// paired-device list. Safe to call repeatedly; it only starts the daemon once.
pub fn refresh(ui: &Rc<Ui>) {
    if ui.peers_started.get() {
        if let Some(d) = ui.syncthingd.borrow().as_ref().cloned() {
            populate(ui, &d);
        }
        return;
    }
    if esp_core::syncthing_path().is_none() {
        ui.peers_id
            .set_text("The P2P engine is not available in this build.");
        return;
    }
    ui.peers_id.set_text("Starting the P2P engine\u{2026}");
    // Syncthingd::start blocks for a couple of seconds booting the daemon; do it off-thread. The
    // handle is Send, so we build it on the worker and wrap it in an Rc back on the UI thread.
    let (tx, rx) = async_channel::bounded(1);
    std::thread::spawn(move || {
        match esp_core::Syncthingd::start() {
            Ok(d) => {
                let _ = tx.send_blocking(Ok(d));
                // The daemon is spawned here with PR_SET_PDEATHSIG, which fires when THIS thread
                // (its parent thread), not the process, exits. So this thread must outlive the
                // daemon: park it for the app's lifetime. It ends when the process does, which is
                // exactly when we want the daemon reaped. One idle thread is a cheap price.
                loop {
                    std::thread::park();
                }
            }
            Err(e) => {
                let _ = tx.send_blocking(Err(e.to_string()));
            }
        }
    });
    let ui = ui.clone();
    glib::spawn_future_local(async move {
        match rx.recv().await {
            Ok(Ok(daemon)) => {
                let d = Rc::new(daemon);
                *ui.syncthingd.borrow_mut() = Some(d.clone());
                ui.peers_started.set(true);
                populate(&ui, &d);
            }
            Ok(Err(e)) => ui.peers_id.set_text(&format!("Could not start the P2P engine: {e}")),
            Err(_) => {}
        }
    });
}

/// Fill the device code, QR and peer list from a running daemon. The `syncthing cli` reads are fast
/// shell-outs, run on the UI thread the same way the app reads rclone config elsewhere.
fn populate(ui: &Rc<Ui>, daemon: &Rc<esp_core::Syncthingd>) {
    let my_id = match daemon.device_id() {
        Ok(id) => id,
        Err(e) => {
            esp_core::diag::error(format!("peers: device_id failed: {e:#}"));
            String::new()
        }
    };
    if my_id.is_empty() {
        ui.peers_id.set_text("Could not read this device's code.");
    } else {
        ui.peers_id.set_text(&my_id);
        let data = my_id.clone();
        ui.peers_qr
            .set_draw_func(move |_area, cr, w, h| draw_qr(cr, w, h, &data));
        ui.peers_qr.queue_draw();
    }

    while let Some(child) = ui.peers_list.first_child() {
        ui.peers_list.remove(&child);
    }
    let peers = daemon.peer_ids().unwrap_or_default();
    if peers.is_empty() {
        let row = adw::ActionRow::builder()
            .title("No devices yet")
            .subtitle("Add a device by its code, or scan this one from your phone.")
            .build();
        row.add_prefix(&gtk::Image::from_icon_name("phone-symbolic"));
        ui.peers_list.append(&row);
        return;
    }
    for pid in peers {
        let short = short_id(&pid);
        let row = adw::ActionRow::builder()
            .title(&short)
            .subtitle("Paired")
            .build();
        row.add_prefix(&gtk::Image::from_icon_name("computer-symbolic"));
        let share = gtk::Button::with_label("Share a folder");
        share.set_valign(gtk::Align::Center);
        let (ui2, pid2) = (ui.clone(), pid.clone());
        share.connect_clicked(move |_| share_folder_dialog(&ui2, &pid2));
        row.add_suffix(&share);
        ui.peers_list.append(&row);
    }
}

/// Dialog to pair a device by its code and a friendly name.
pub fn add_device_dialog(ui: &Rc<Ui>) {
    let name = adw::EntryRow::builder().title("Name (e.g. My phone)").build();
    let code = adw::EntryRow::builder().title("Device code").build();
    let list = gtk::ListBox::new();
    list.add_css_class("boxed-list");
    list.set_selection_mode(gtk::SelectionMode::None);
    list.append(&name);
    list.append(&code);

    let dialog = adw::AlertDialog::new(
        Some("Add a device"),
        Some("Open Syncthing on the other device, copy its code, and paste it here."),
    );
    dialog.set_extra_child(Some(&list));
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("add", "Add");
    dialog.set_response_appearance("add", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("add"));
    dialog.set_close_response("cancel");

    let ui2 = ui.clone();
    dialog.connect_response(None, move |_, resp| {
        if resp != "add" {
            return;
        }
        let (n, c) = (name.text().trim().to_string(), code.text().trim().to_string());
        if c.is_empty() {
            ui2.error("Paste the other device's code.");
            return;
        }
        let label = if n.is_empty() { "A device".to_string() } else { n };
        let daemon = match ui2.ensure_syncthingd() {
            Ok(d) => d,
            Err(e) => {
                ui2.error(&format!("Could not start the P2P engine: {e}"));
                return;
            }
        };
        ui2.peers_started.set(true);
        match daemon.add_peer(&c, &label) {
            Ok(()) => {
                populate(&ui2, &daemon);
                ui2.toast(&format!("Added {label}"));
            }
            Err(e) => ui2.error(&format!("Could not add that device: {e}")),
        }
    });
    dialog.present(Some(&ui.window));
}

/// Pick a local folder and share it with `peer_id`.
fn share_folder_dialog(ui: &Rc<Ui>, peer_id: &str) {
    let chooser = gtk::FileDialog::builder()
        .title("Share a folder with this device")
        .build();
    if let Some(dir) = crate::default_save_dir() {
        chooser.set_initial_folder(Some(&gtk::gio::File::for_path(dir)));
    }
    let (ui2, peer) = (ui.clone(), peer_id.to_string());
    chooser.select_folder(
        Some(&ui.window),
        gtk::gio::Cancellable::NONE,
        move |res| {
            let Ok(file) = res else { return };
            let Some(path) = file.path() else { return };
            let path = path.to_string_lossy().into_owned();
            let label = std::path::Path::new(&path)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "Shared".to_string());
            let folder_id = folder_id_for(&path);
            let daemon = match ui2.syncthingd.borrow().as_ref().cloned() {
                Some(d) => d,
                None => {
                    ui2.error("The P2P engine is not running.");
                    return;
                }
            };
            match daemon.share_folder(&folder_id, &label, &path, &peer) {
                Ok(()) => ui2.toast(&format!("Sharing \u{201c}{label}\u{201d}")),
                Err(e) => ui2.error(&format!("Could not share that folder: {e}")),
            }
        },
    );
}

/// A stable, path-derived folder id so re-sharing the same folder reuses one Syncthing entry.
fn folder_id_for(path: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut h);
    format!("oort-{:x}", h.finish())
}

/// A short, human form of a long device code for the list (first group + ellipsis).
fn short_id(id: &str) -> String {
    match id.split_once('-') {
        Some((head, _)) => format!("{head}\u{2026}"),
        None => id.chars().take(7).collect::<String>() + "\u{2026}",
    }
}

/// Draw a QR of `data` centered in the drawing area: a white card with near-black modules and a
/// quiet-zone border, so a phone camera reads it cleanly.
fn draw_qr(cr: &gtk::cairo::Context, w: i32, h: i32, data: &str) {
    if data.is_empty() {
        return;
    }
    let code = match qrcode::QrCode::new(data.as_bytes()) {
        Ok(c) => c,
        Err(_) => return,
    };
    let modules = code.width();
    let colors = code.to_colors();
    let quiet = 3usize;
    let total = modules + quiet * 2;
    let size = w.min(h) as f64;
    let scale = size / total as f64;
    // White card.
    cr.set_source_rgb(1.0, 1.0, 1.0);
    cr.rectangle(0.0, 0.0, size, size);
    let _ = cr.fill();
    // Near-black ink (the night-sky deep indigo), one rectangle per dark module.
    cr.set_source_rgb(0.05, 0.04, 0.10);
    for y in 0..modules {
        for x in 0..modules {
            if colors[y * modules + x] != qrcode::Color::Light {
                let px = (x + quiet) as f64 * scale;
                let py = (y + quiet) as f64 * scale;
                cr.rectangle(px, py, scale.ceil(), scale.ceil());
            }
        }
    }
    let _ = cr.fill();
}
