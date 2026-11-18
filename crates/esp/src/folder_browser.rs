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

    let spinner = gtk::Spinner::new();
    spinner.set_halign(gtk::Align::Center);
    spinner.set_margin_top(16);

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
    body.append(&scroll);

    let tv = adw::ToolbarView::new();
    tv.add_top_bar(&header);
    tv.set_content(Some(&body));
    dialog.set_child(Some(&tv));

    // Recursive loader held in a cell so row callbacks can re-enter it for navigation.
    let load: Rc<RefCell<Option<Loader>>> = Rc::new(RefCell::new(None));
    let load_impl: Loader = {
        let (fs, cur, ui, list, spinner, crumb, load) = (
            fs.clone(),
            cur.clone(),
            ui.clone(),
            list.clone(),
            spinner.clone(),
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
            spinner.start();

            let (tx, rx) = async_channel::bounded(1);
            let (fs_t, sub_t) = (fs.clone(), sub.clone());
            std::thread::spawn(move || {
                let _ = tx.send_blocking(esp_core::list_dirs(port, &fs_t, &sub_t));
            });

            let (list_c, spinner_c, ui_c, load_c, sub_c) = (
                list.clone(),
                spinner.clone(),
                ui.clone(),
                load.clone(),
                sub.clone(),
            );
            glib::spawn_future_local(async move {
                let res = rx.recv().await;
                spinner_c.stop();
                spinner_c.set_visible(false);
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
                    Ok(Err(e)) => ui_c.toast(&format!("Could not list folders: {e}")),
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
