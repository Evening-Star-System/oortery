//! A folder browser: walk a remote's (or this computer's) directory tree and pick a folder.
//! Each listing runs on a worker thread, so a slow cloud lookup shows a spinner instead of
//! freezing the app. `fs` is the root ("remote:" or "/"); navigation is by subpath.

use crate::Ui;
use adw::prelude::*;
use gtk::glib;
use std::cell::RefCell;
use std::rc::Rc;

/// A navigation callback: given a subpath, list and show it.
type Loader = Rc<dyn Fn(String)>;

/// Present the browser over the window. `start` is the subpath to open at (empty = root).
/// `on_pick` receives the chosen subpath when the user confirms.
pub fn present(ui: &Rc<Ui>, fs: String, start: String, on_pick: Loader) {
    let port = match ui.ensure_rcd() {
        Ok(r) => r.port(),
        Err(e) => {
            ui.toast(&format!("Could not start the engine: {e}"));
            return;
        }
    };

    let dialog = adw::Dialog::builder()
        .title("Choose a folder")
        .content_width(520)
        .content_height(560)
        .build();

    let cur = Rc::new(RefCell::new(start));

    let header = adw::HeaderBar::new();
    let use_btn = gtk::Button::with_label("Use this folder");
    use_btn.add_css_class("suggested-action");
    header.pack_end(&use_btn);

    let crumb = gtk::Label::new(None);
    crumb.add_css_class("dim");
    crumb.add_css_class("caption");
    crumb.set_xalign(0.0);
    crumb.set_ellipsize(gtk::pango::EllipsizeMode::Start);
    crumb.set_margin_start(4);

    let spinner = crate::spinner::celestial();
    spinner.set_margin_top(16);
    let status = gtk::Label::new(None);
    status.add_css_class("dim");
    status.add_css_class("caption");
    status.set_margin_top(4);
    status.set_wrap(true);
    status.set_justify(gtk::Justification::Center);

    let list = gtk::ListBox::new();
    list.add_css_class("boxed-list");
    list.set_selection_mode(gtk::SelectionMode::None);
    let scroll = gtk::ScrolledWindow::new();
    scroll.set_vexpand(true);
    scroll.set_child(Some(&list));

    let body = gtk::Box::new(gtk::Orientation::Vertical, 8);
    body.set_margin_top(8);
    body.set_margin_bottom(12);
    body.set_margin_start(12);
    body.set_margin_end(12);
    body.append(&crumb);
    body.append(&spinner);
    body.append(&status);
    body.append(&scroll);

    let tv = adw::ToolbarView::new();
    tv.add_top_bar(&header);
    tv.set_content(Some(&body));
    dialog.set_child(Some(&tv));

    // Recursive loader held in a cell so row callbacks can re-enter it for navigation.
    let load: Rc<RefCell<Option<Loader>>> = Rc::new(RefCell::new(None));
    let load_impl: Loader = {
        let (fs, cur, list, spinner, status, crumb, load) = (
            fs.clone(),
            cur.clone(),
            list.clone(),
            spinner.clone(),
            status.clone(),
            crumb.clone(),
            load.clone(),
        );
        Rc::new(move |sub: String| {
            *cur.borrow_mut() = sub.clone();
            crumb.set_text(&format!("{fs}{sub}"));
            while let Some(c) = list.first_child() {
                list.remove(&c);
            }
            spinner.set_visible(true);
            status.set_text("Loading folders\u{2026}");
            status.set_visible(true);

            // A slow cloud (a large Google Drive listing for the first time) can take a while;
            // reassure instead of looking frozen, and never leave a bare spinner forever.
            let done = Rc::new(std::cell::Cell::new(false));
            {
                let (status_s, done_s) = (status.clone(), done.clone());
                glib::timeout_add_local_once(std::time::Duration::from_secs(18), move || {
                    if !done_s.get() {
                        status_s.set_text(
                            "Still loading. A large cloud can be slow the first time it is listed.",
                        );
                    }
                });
            }

            let (tx, rx) = async_channel::bounded(1);
            let (fs_t, sub_t) = (fs.clone(), sub.clone());
            std::thread::spawn(move || {
                let _ = tx.send_blocking(esp_core::list_dirs(port, &fs_t, &sub_t));
            });

            let (list_c, spinner_c, status_c, load_c, sub_c, done_c) = (
                list.clone(),
                spinner.clone(),
                status.clone(),
                load.clone(),
                sub.clone(),
                done.clone(),
            );
            glib::spawn_future_local(async move {
                let res = rx.recv().await;
                done_c.set(true);
                spinner_c.set_visible(false);
                status_c.set_visible(false);
                match res {
                    Ok(Ok(dirs)) => {
                        if !sub_c.is_empty() {
                            let row = adw::ActionRow::builder()
                                .title("Up one level")
                                .activatable(true)
                                .build();
                            row.add_prefix(&gtk::Image::from_icon_name("go-up-symbolic"));
                            let load_up = load_c.clone();
                            let parent = parent_of(&sub_c);
                            row.connect_activated(move |_| {
                                if let Some(f) = load_up.borrow().clone() {
                                    f(parent.clone());
                                }
                            });
                            list_c.append(&row);
                        }
                        for d in dirs {
                            let row = adw::ActionRow::builder()
                                .title(&d.name)
                                .activatable(true)
                                .build();
                            row.add_prefix(&gtk::Image::from_icon_name("folder-symbolic"));
                            row.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
                            let load_d = load_c.clone();
                            let path = d.path.clone();
                            row.connect_activated(move |_| {
                                if let Some(f) = load_d.borrow().clone() {
                                    f(path.clone());
                                }
                            });
                            list_c.append(&row);
                        }
                        if list_c.first_child().is_none() {
                            let row = adw::ActionRow::builder()
                                .title("No subfolders here")
                                .subtitle("Use this folder, or go up a level.")
                                .build();
                            list_c.append(&row);
                        }
                    }
                    // Show the failure inline with a Retry, so the user is not stuck.
                    Ok(Err(e)) => {
                        let row = adw::ActionRow::builder()
                            .title("Could not load folders")
                            .subtitle(e.to_string())
                            .build();
                        row.add_prefix(&gtk::Image::from_icon_name("dialog-warning-symbolic"));
                        let retry = gtk::Button::with_label("Retry");
                        retry.set_valign(gtk::Align::Center);
                        let (load_r, sub_r) = (load_c.clone(), sub_c.clone());
                        retry.connect_clicked(move |_| {
                            if let Some(f) = load_r.borrow().clone() {
                                f(sub_r.clone());
                            }
                        });
                        row.add_suffix(&retry);
                        list_c.append(&row);
                    }
                    Err(_) => {}
                }
            });
        })
    };
    *load.borrow_mut() = Some(load_impl.clone());

    let cur_for_use = cur.clone();
    let dlg_for_use = dialog.clone();
    use_btn.connect_clicked(move |_| {
        on_pick(cur_for_use.borrow().clone());
        dlg_for_use.close();
    });

    let initial = cur.borrow().clone();
    load_impl(initial);
    dialog.present(Some(&ui.window));
}

/// The parent subpath ("a/b/c" -> "a/b", "a" -> "").
fn parent_of(sub: &str) -> String {
    match sub.rfind('/') {
        Some(i) => sub[..i].to_string(),
        None => String::new(),
    }
}
