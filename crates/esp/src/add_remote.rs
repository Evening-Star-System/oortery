//! The add-remote flow: pick any backend rclone supports, then connect it. OAuth clouds
//! (Drive, Dropbox, OneDrive, ...) sign in with one tap in the browser; config-value
//! backends (S3, SFTP, WebDAV, B2, and many more) fill a short form. Both end up connected.

use crate::{refresh, Ui};
use adw::prelude::*;
use gtk::glib;
use std::cell::RefCell;
use std::rc::Rc;

pub fn present(ui: &Rc<Ui>) {
    let dialog = adw::Dialog::builder()
        .title("Add a remote")
        .content_width(560)
        .content_height(640)
        .build();
    let nav = adw::NavigationView::new();
    nav.add(&picker_page(ui, &nav, &dialog));
    dialog.set_child(Some(&nav));
    dialog.present(Some(&ui.window));

    // Headless aid: jump straight to a provider's config form for screenshots.
    if let Some(name) = crate::aid("ESP_DIALOG_PROVIDER") {
        if let Some(p) = ui.providers.iter().find(|p| p.name == name) {
            nav.push(&config_page(ui, p, &dialog));
        }
    }
}

fn first_line(s: &str) -> &str {
    s.lines().next().unwrap_or(s)
}

fn picker_page(
    ui: &Rc<Ui>,
    nav: &adw::NavigationView,
    dialog: &adw::Dialog,
) -> adw::NavigationPage {
    let search = gtk::SearchEntry::new();
    search.set_placeholder_text(Some(&format!("Search {} providers...", ui.providers.len())));

    let list = gtk::ListBox::new();
    list.add_css_class("boxed-list");
    list.set_selection_mode(gtk::SelectionMode::None);

    for p in &ui.providers {
        let row = adw::ActionRow::builder()
            .title(&p.name)
            .subtitle(first_line(&p.description))
            .activatable(true)
            .build();
        row.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
        let ui2 = ui.clone();
        let nav2 = nav.clone();
        let dlg2 = dialog.clone();
        let prov = p.clone();
        row.connect_activated(move |_| {
            nav2.push(&config_page(&ui2, &prov, &dlg2));
        });
        list.append(&row);
    }

    // Live search over name + description.
    let query = Rc::new(RefCell::new(String::new()));
    let ui_f = ui.clone();
    let query_f = query.clone();
    list.set_filter_func(move |row| {
        let q = query_f.borrow();
        if q.is_empty() {
            return true;
        }
        match ui_f.providers.get(row.index() as usize) {
            Some(p) => {
                p.name.to_lowercase().contains(q.as_str())
                    || p.description.to_lowercase().contains(q.as_str())
            }
            None => true,
        }
    });
    let list_s = list.clone();
    search.connect_search_changed(move |e| {
        *query.borrow_mut() = e.text().to_lowercase();
        list_s.invalidate_filter();
    });

    let scroll = gtk::ScrolledWindow::new();
    scroll.set_vexpand(true);
    scroll.set_child(Some(&list));

    let body = gtk::Box::new(gtk::Orientation::Vertical, 10);
    body.set_margin_top(12);
    body.set_margin_bottom(12);
    body.set_margin_start(12);
    body.set_margin_end(12);
    body.append(&search);
    body.append(&scroll);

    let tv = adw::ToolbarView::new();
    tv.add_top_bar(&adw::HeaderBar::new());
    tv.set_content(Some(&body));

    adw::NavigationPage::builder()
        .title("Add a remote")
        .tag("picker")
        .child(&tv)
        .build()
}

fn config_page(
    ui: &Rc<Ui>,
    provider: &esp_core::Provider,
    dialog: &adw::Dialog,
) -> adw::NavigationPage {
    let prefs = adw::PreferencesPage::new();
    let pretty = first_line(&provider.description).to_string();

    let gname = adw::PreferencesGroup::builder().title("New remote").build();
    let name_row = adw::EntryRow::builder().title("Name").build();
    gname.add(&name_row);
    prefs.add(&gname);

    let header = adw::HeaderBar::new();

    if provider.needs_oauth() {
        oauth_page(ui, dialog, provider, &pretty, &prefs, &header, &name_row);
    } else {
        config_value_page(ui, dialog, provider, &prefs, &header, &name_row);
    }

    let scroll = gtk::ScrolledWindow::new();
    scroll.set_vexpand(true);
    scroll.set_child(Some(&prefs));
    let tv = adw::ToolbarView::new();
    tv.add_top_bar(&header);
    tv.set_content(Some(&scroll));

    adw::NavigationPage::builder()
        .title(&pretty)
        .child(&tv)
        .build()
}

/// OAuth cloud: just a name, then one tap signs in through the browser.
fn oauth_page(
    ui: &Rc<Ui>,
    dialog: &adw::Dialog,
    provider: &esp_core::Provider,
    pretty: &str,
    prefs: &adw::PreferencesPage,
    header: &adw::HeaderBar,
    name_row: &adw::EntryRow,
) {
    let g = adw::PreferencesGroup::new();
    let row = adw::ActionRow::builder()
        .title("Sign in securely")
        .subtitle(format!(
            "You'll sign in on {pretty}'s own page in your browser. Oortery never sees your password."
        ))
        .build();
    row.add_prefix(&gtk::Image::from_icon_name("dialog-password-symbolic"));
    g.add(&row);
    prefs.add(&g);

    let signin = gtk::Button::with_label(&format!("Sign in with {pretty}"));
    signin.add_css_class("suggested-action");
    header.pack_end(&signin);

    let ui2 = ui.clone();
    let dlg2 = dialog.clone();
    let backend = provider.name.clone();
    let pretty_own = pretty.to_string();
    let name_row2 = name_row.clone();
    signin.connect_clicked(move |btn| {
        let name = name_row2.text().trim().to_string();
        if name.is_empty() {
            ui2.toast("Give this cloud a name first.");
            return;
        }
        let port = match ui2.ensure_rcd() {
            Ok(r) => r.port(),
            Err(e) => {
                ui2.toast(&format!("Could not start the engine: {e}"));
                return;
            }
        };
        btn.set_sensitive(false);
        btn.set_label("Signing in...");
        ui2.toast(&format!(
            "Opening your browser to sign in to {pretty_own}..."
        ));

        // The OAuth flow blocks until the user finishes, so run it off the UI thread.
        let (tx, rx) = async_channel::bounded(1);
        let (n, b) = (name.clone(), backend.clone());
        std::thread::spawn(move || {
            let _ = tx.send_blocking(esp_core::connect::oauth_connect(port, &n, &b));
        });

        let ui3 = ui2.clone();
        let dlg3 = dlg2.clone();
        let btn2 = btn.clone();
        let pretty2 = pretty_own.clone();
        glib::spawn_future_local(async move {
            match rx.recv().await {
                Ok(Ok(())) => {
                    dlg3.close();
                    refresh(&ui3);
                    ui3.toast(&format!("Connected to {pretty2}"));
                }
                Ok(Err(e)) => {
                    btn2.set_sensitive(true);
                    btn2.set_label(&format!("Sign in with {pretty2}"));
                    ui3.error(&format!("Sign-in did not finish: {e}"));
                }
                Err(_) => {
                    btn2.set_sensitive(true);
                    btn2.set_label(&format!("Sign in with {pretty2}"));
                    ui3.toast("Sign-in was interrupted.");
                }
            }
        });
    });
}

/// Config-value backend: a short form of its settings, then Create.
fn config_value_page(
    ui: &Rc<Ui>,
    dialog: &adw::Dialog,
    provider: &esp_core::Provider,
    prefs: &adw::PreferencesPage,
    header: &adw::HeaderBar,
    name_row: &adw::EntryRow,
) {
    let gopts = adw::PreferencesGroup::builder().title("Settings").build();
    // (field name, required, the entry) so Create can refuse before making a broken empty remote.
    let mut fields: Vec<(String, bool, gtk::Editable)> = Vec::new();
    for o in provider.basic_options() {
        if o.name == "token" {
            continue; // OAuth internals, not a manual field
        }
        let title = if o.required {
            format!("{} (required)", o.name)
        } else {
            o.name.clone()
        };
        if o.is_password || o.sensitive {
            let row = adw::PasswordEntryRow::builder().title(&title).build();
            gopts.add(&row);
            fields.push((o.name.clone(), o.required, row.upcast::<gtk::Editable>()));
        } else {
            let row = adw::EntryRow::builder().title(&title).build();
            gopts.add(&row);
            fields.push((o.name.clone(), o.required, row.upcast::<gtk::Editable>()));
        }
    }
    prefs.add(&gopts);

    let create = gtk::Button::with_label("Create");
    create.add_css_class("suggested-action");
    header.pack_end(&create);

    let ui2 = ui.clone();
    let dlg2 = dialog.clone();
    let backend = provider.name.clone();
    let name_row2 = name_row.clone();
    create.connect_clicked(move |_| {
        let name = name_row2.text().trim().to_string();
        if name.is_empty() {
            ui2.error("Give the remote a name.");
            return;
        }
        // Refuse before creating a broken, empty remote: rclone does not validate required fields,
        // so a blank one would otherwise be saved and reported as a success.
        if let Some((field, _, _)) = fields
            .iter()
            .find(|(_, required, e)| *required && e.text().trim().is_empty())
        {
            ui2.error(&format!("\u{201c}{field}\u{201d} is required."));
            return;
        }
        let params: Vec<(String, String)> = fields
            .iter()
            .map(|(k, _, e)| (k.clone(), e.text().to_string()))
            .collect();
        match esp_core::create_remote(&name, &backend, &params) {
            Ok(()) => {
                dlg2.close();
                refresh(&ui2);
                ui2.toast(&format!("Added {name}"));
            }
            Err(e) => ui2.error(&format!("Could not add: {e}")),
        }
    });
}
