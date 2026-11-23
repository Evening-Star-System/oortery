//! The Diagnostics dialog: this session's local log, shown so the user can read, copy, or save it to
//! send for support. Nothing is transmitted from here; saving writes a plain file the user shares by
//! hand. The log carries names and paths but never passwords, so it is safe to share for help.

use crate::Ui;
use adw::prelude::*;
use gtk::gio;
use std::rc::Rc;

pub fn present(ui: &Rc<Ui>) {
    let dialog = adw::Dialog::builder()
        .title("Diagnostics")
        .content_width(660)
        .content_height(560)
        .build();

    let toolbar = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();
    let copy_btn = crate::icon_button("edit-copy-symbolic", "Copy all");
    let save_btn = gtk::Button::with_label("Save...");
    header.pack_start(&copy_btn);
    header.pack_end(&save_btn);
    toolbar.add_top_bar(&header);

    let body = gtk::Box::new(gtk::Orientation::Vertical, 10);
    body.set_margin_top(12);
    body.set_margin_bottom(12);
    body.set_margin_start(12);
    body.set_margin_end(12);

    let note = gtk::Label::new(Some(
        "This log stays on your computer and is never sent automatically. It can contain file and \
         remote names and paths, but never passwords. Copy or save it and share it only if you want help.",
    ));
    note.set_wrap(true);
    note.set_xalign(0.0);
    note.add_css_class("dim");
    body.append(&note);

    let text = esp_core::diag::export_text();
    let shown = if text.is_empty() {
        "No diagnostics recorded yet.".to_string()
    } else {
        text.clone()
    };
    let view = gtk::TextView::new();
    view.set_editable(false);
    view.set_cursor_visible(false);
    view.set_monospace(true);
    view.set_wrap_mode(gtk::WrapMode::WordChar);
    view.buffer().set_text(&shown);
    let scroll = gtk::ScrolledWindow::new();
    scroll.set_vexpand(true);
    scroll.set_child(Some(&view));
    scroll.add_css_class("card");
    body.append(&scroll);

    toolbar.set_content(Some(&body));
    dialog.set_child(Some(&toolbar));

    {
        let (ui_c, text_c) = (ui.clone(), text.clone());
        copy_btn.connect_clicked(move |btn| {
            btn.clipboard().set_text(&text_c);
            ui_c.toast("Diagnostics copied to the clipboard");
        });
    }
    {
        let ui_s = ui.clone();
        let text_s = text.clone();
        save_btn.connect_clicked(move |_| {
            let chooser = gtk::FileDialog::builder()
                .title("Save diagnostics")
                .initial_name("oortery-diagnostics.txt")
                .build();
            if let Some(dir) = crate::default_save_dir() {
                chooser.set_initial_folder(Some(&gio::File::for_path(dir)));
            }
            let (ui_cb, text_cb) = (ui_s.clone(), text_s.clone());
            chooser.save(Some(&ui_s.window), gio::Cancellable::NONE, move |res| {
                let Ok(file) = res else { return };
                let Some(path) = file.path() else { return };
                match std::fs::write(&path, text_cb.as_bytes()) {
                    Ok(()) => ui_cb.toast(&format!("Saved to {}", path.display())),
                    Err(e) => ui_cb.error(&format!("Could not save diagnostics: {e}")),
                }
            });
        });
    }

    dialog.present(Some(&ui.window));
}
