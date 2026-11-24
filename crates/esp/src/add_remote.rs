//! The add-remote flow: pick any backend rclone supports, then connect it. OAuth clouds
//! (Drive, Dropbox, OneDrive, ...) sign in with one tap in the browser; config-value
//! backends (S3, SFTP, WebDAV, B2, and many more) fill a short form. Both end up connected.

use crate::Ui;
use adw::prelude::*;
use gtk::glib;
use std::cell::RefCell;
use std::rc::Rc;

/// The top consumer clouds shown under "Popular", by real-world popularity. Six keeps the 2-column
/// grid square (3 even rows). Each maps to the rclone provider of the same name (which decides OAuth
/// vs form). Object stores (S3/R2/B2) live in their own section, not here. Easy to re-order.
const POPULAR: &[&str] = &["drive", "dropbox", "onedrive", "box", "pcloud", "mega"];

pub fn present(ui: &Rc<Ui>) {
    let dialog = adw::Dialog::builder()
        .title("Add a remote")
        .content_width(560)
        .content_height(700)
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
    // Headless aid: jump straight to a preset form (e.g. ESP_PRESET=r2) for screenshots.
    if let Some(id) = crate::aid("ESP_PRESET") {
        if let Some(p) = esp_core::presets::preset(&id) {
            nav.push(&preset_page(ui, p, &dialog));
        }
    }
}

fn first_line(s: &str) -> &str {
    s.lines().next().unwrap_or(s)
}

/// A big, friendly provider card for the Popular grid: an icon, the human name, and a plain
/// one-line description of what connecting it involves. Warmer and more inviting for a newcomer
/// than a dense list row.
fn provider_card(icon: &str, title: &str, desc: &str) -> gtk::Button {
    // Compact horizontal layout: a small icon beside a bold name over a dim one-liner. Reads like a
    // tidy list item rather than a tall, blocky card.
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    row.set_margin_top(8);
    row.set_margin_bottom(8);
    row.set_margin_start(10);
    row.set_margin_end(10);
    let img = gtk::Image::from_icon_name(icon);
    img.set_pixel_size(18);
    img.set_valign(gtk::Align::Center);
    img.add_css_class("dim");
    let text = gtk::Box::new(gtk::Orientation::Vertical, 0);
    text.set_valign(gtk::Align::Center);
    text.set_hexpand(true);
    let name = gtk::Label::new(Some(title));
    name.add_css_class("heading");
    name.set_xalign(0.0);
    name.set_ellipsize(gtk::pango::EllipsizeMode::End);
    let sub = gtk::Label::new(Some(desc));
    sub.add_css_class("dim");
    sub.add_css_class("caption");
    sub.set_xalign(0.0);
    sub.set_ellipsize(gtk::pango::EllipsizeMode::End);
    text.append(&name);
    text.append(&sub);
    row.append(&img);
    row.append(&text);
    let btn = gtk::Button::builder().child(&row).build();
    btn.add_css_class("card");
    btn.set_hexpand(true);
    btn
}

/// A plain-language note for backends with a known gotcha, shown above the form so a new user is
/// not left guessing.
fn backend_hint(backend: &str) -> Option<&'static str> {
    match backend {
        "protondrive" => Some(
            "If you use two-factor authentication, put your current 6-digit code in the 2fa field. \
             A separate mailbox password (two-password accounts) goes under Advanced settings. If \
             sign-in keeps failing with a CAPTCHA or security block, turn off Proton Sentinel in \
             your Proton account (Settings, then Security): it blocks non-browser apps like this \
             one. Sign in once at proton.me in a browser first, then try again here.",
        ),
        "mega" => Some(
            "Use your MEGA email and password. If you have two-factor authentication on, add your \
             current 6-digit code in the 2fa field.",
        ),
        "filen" => Some(
            "Filen needs your email and password plus an API key. Generate the API key in Filen \
             under Settings, Account, then paste it here.",
        ),
        "pikpak" => Some(
            "Use your PikPak username and password. If sign-in is refused, finish a login once in \
             the PikPak app or website first (it can require a one-time human check), then retry.",
        ),
        "internxt" => Some(
            "Use your Internxt email and password. Your files stay end-to-end encrypted. This method \
             does not support two-factor authentication, so if you have 2FA on, turn it off for now.",
        ),
        "b2" => Some("Account is your Backblaze keyID, and Key is the applicationKey."),
        "webdav" => Some(
            "For Nextcloud or ownCloud, set vendor accordingly and use the WebDAV URL that ends in \
             /remote.php/dav/files/USERNAME/.",
        ),
        "sftp" => Some("Leave the password blank to sign in with an SSH key file instead."),
        _ => None,
    }
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
        // Lead with the human description ("Amazon S3"), not the raw rclone id ("s3"), which read as
        // engineer jargon to a normal user. Keep the id as the dim subtitle for those who know it.
        let human = first_line(&p.description);
        let title = if human.is_empty() {
            p.name.as_str()
        } else {
            human
        };
        let row = adw::ActionRow::builder()
            .title(title)
            .subtitle(&p.name)
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

    // The handful of providers most people actually reach for, offered first so the long list is
    // optional. OAuth clouds route to one-tap sign-in; the object store (B2) uses its easy preset
    // form. Ordered by real-world popularity; trivial to re-order or extend.
    let popular = gtk::Label::new(Some("Popular"));
    popular.set_xalign(0.0);
    popular.add_css_class("heading");
    let presets_list = gtk::FlowBox::new();
    presets_list.set_selection_mode(gtk::SelectionMode::None);
    presets_list.set_min_children_per_line(2);
    presets_list.set_max_children_per_line(2);
    presets_list.set_column_spacing(10);
    presets_list.set_row_spacing(10);
    presets_list.set_homogeneous(true);
    for key in POPULAR {
        // Prefer a stupid-easy preset if one exists for this key (e.g. B2); otherwise the provider.
        if let Some(preset) = esp_core::presets::preset(key) {
            let card = provider_card("folder-remote-symbolic", preset.name, preset.blurb);
            let (ui2, nav2, dlg2) = (ui.clone(), nav.clone(), dialog.clone());
            card.connect_clicked(move |_| {
                nav2.push(&preset_page(&ui2, preset, &dlg2));
            });
            presets_list.append(&card);
        } else if let Some(p) = ui.providers.iter().find(|p| &p.name == key).cloned() {
            let subtitle = if p.needs_oauth() {
                "Sign in with your browser"
            } else {
                "Fill in a short form"
            };
            let card = provider_card(
                "folder-remote-symbolic",
                first_line(&p.description),
                subtitle,
            );
            let (ui2, nav2, dlg2) = (ui.clone(), nav.clone(), dialog.clone());
            card.connect_clicked(move |_| {
                nav2.push(&config_page(&ui2, &p, &dlg2));
            });
            presets_list.append(&card);
        }
    }
    // Object storage (S3, R2, B2, and any S3-compatible store): the guided presets, which assemble
    // the real rclone config from a short, bucket-scoped form. Surfaced on their own so R2 and S3 are
    // reachable through the easy flow instead of falling through to the raw multi-field `s3` backend.
    let obj_label = gtk::Label::new(Some("Object storage"));
    obj_label.set_xalign(0.0);
    obj_label.add_css_class("heading");
    let obj_list = gtk::FlowBox::new();
    obj_list.set_selection_mode(gtk::SelectionMode::None);
    obj_list.set_min_children_per_line(1);
    obj_list.set_max_children_per_line(2);
    obj_list.set_column_spacing(10);
    obj_list.set_row_spacing(10);
    obj_list.set_homogeneous(true);
    for preset in esp_core::presets::PRESETS {
        let card = provider_card("drive-harddisk-symbolic", preset.name, preset.blurb);
        let (ui2, nav2, dlg2) = (ui.clone(), nav.clone(), dialog.clone());
        card.connect_clicked(move |_| {
            nav2.push(&preset_page(&ui2, preset, &dlg2));
        });
        obj_list.append(&card);
    }
    let obj_exp = gtk::Expander::new(None);
    obj_exp.set_label_widget(Some(&obj_label));
    obj_exp.set_expanded(true);
    let obj_body = gtk::Box::new(gtk::Orientation::Vertical, 10);
    obj_body.set_margin_top(8);
    obj_body.append(&obj_list);
    obj_exp.set_child(Some(&obj_body));

    let all_label = gtk::Label::new(Some("Or pick any provider"));
    all_label.set_xalign(0.0);
    all_label.add_css_class("heading");
    all_label.set_margin_top(6);

    // Popular is collapsible so a user who wants the full A-Z list can fold it away and give the
    // scrolling list the whole pane. Open by default (most people take a popular one).
    let popular_exp = gtk::Expander::new(None);
    popular_exp.set_label_widget(Some(&popular));
    popular_exp.set_expanded(true);
    let popular_body = gtk::Box::new(gtk::Orientation::Vertical, 10);
    popular_body.set_margin_top(8);
    popular_body.append(&presets_list);
    popular_exp.set_child(Some(&popular_body));

    let body = gtk::Box::new(gtk::Orientation::Vertical, 10);
    body.set_margin_top(12);
    body.set_margin_bottom(12);
    body.set_margin_start(12);
    body.set_margin_end(12);
    body.append(&popular_exp);
    body.append(&obj_exp);
    body.append(&all_label);
    body.append(&search);
    body.append(&list);

    // The whole picker scrolls as one unit (Popular + Object storage + the full provider list), so
    // adding sections never overflows the dialog; it just scrolls.
    let body_scroll = gtk::ScrolledWindow::new();
    body_scroll.set_vexpand(true);
    body_scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    body_scroll.set_child(Some(&body));

    let tv = adw::ToolbarView::new();
    tv.add_top_bar(&adw::HeaderBar::new());
    tv.set_content(Some(&body_scroll));

    adw::NavigationPage::builder()
        .title("Add a remote")
        .tag("picker")
        .child(&tv)
        .build()
}

/// A stupid-easy preset (Cloudflare R2, Amazon S3, any S3-compatible, Backblaze B2): a short form of
/// only the essentials, which assembles the full rclone config. These backends do not log in at
/// create time, so a correct preset connects first try.
fn preset_page(
    ui: &Rc<Ui>,
    preset: &'static esp_core::presets::Preset,
    dialog: &adw::Dialog,
) -> adw::NavigationPage {
    let prefs = adw::PreferencesPage::new();
    let header = adw::HeaderBar::new();

    let gname = adw::PreferencesGroup::builder().title("New remote").build();
    let name_row = adw::EntryRow::builder().title("Name").build();
    // Pre-fill a sensible name so connecting is not gated on inventing one first; still editable.
    name_row.set_text(preset.name);
    gname.add(&name_row);
    prefs.add(&gname);

    // A handheld "set it up" section: the plain-language note, then a numbered walkthrough, so a
    // first-timer knows exactly where to click and what each field wants.
    let steps_group = adw::PreferencesGroup::builder()
        .title("Set it up")
        .description(preset.help)
        .build();
    let steps_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
    steps_box.set_margin_top(4);
    steps_box.set_margin_bottom(4);
    for (i, step) in preset.steps.iter().enumerate() {
        let label = gtk::Label::new(Some(&format!("{}.  {step}", i + 1)));
        label.set_wrap(true);
        label.set_xalign(0.0);
        label.add_css_class("dim");
        steps_box.append(&label);
    }
    if !preset.steps.is_empty() {
        steps_group.add(&steps_box);
        prefs.add(&steps_group);
    }

    let gopts = adw::PreferencesGroup::builder()
        .title("Your keys")
        .description(
            "Your provider shows the secret only once. Copy it before you leave that page.",
        )
        .build();
    // (key, label, required, entry)
    let mut fields: Vec<(String, String, bool, gtk::Editable)> = Vec::new();
    for f in preset.fields {
        let title = if f.required {
            format!("{} (required)", f.label)
        } else {
            f.label.to_string()
        };
        let editable: gtk::Editable = if f.secret {
            let row = adw::PasswordEntryRow::builder().title(&title).build();
            gopts.add(&row);
            row.upcast()
        } else {
            let row = adw::EntryRow::builder().title(&title).build();
            gopts.add(&row);
            row.upcast()
        };
        fields.push((f.key.to_string(), f.label.to_string(), f.required, editable));
    }
    prefs.add(&gopts);

    // A visible connection result, so a wrong key shows a plain reason inline instead of the remote
    // being made, silently tested, deleted, and reported only as a fleeting toast.
    let status = gtk::Label::new(None);
    status.set_wrap(true);
    status.set_xalign(0.0);
    status.set_visible(false);
    let gstatus = adw::PreferencesGroup::new();
    gstatus.add(&status);
    prefs.add(&gstatus);

    let create = gtk::Button::with_label("Connect");
    create.add_css_class("suggested-action");
    header.pack_end(&create);

    let (ui2, dlg2, preset_id, name_row2) =
        (ui.clone(), dialog.clone(), preset.id, name_row.clone());
    create.connect_clicked(move |btn| {
        let name = name_row2.text().trim().to_string();
        if name.is_empty() {
            ui2.error("Give the remote a name.");
            return;
        }
        if let Some((_, label, _, _)) = fields
            .iter()
            .find(|(_, _, required, e)| *required && e.text().trim().is_empty())
        {
            ui2.error(&format!("\u{201c}{label}\u{201d} is required."));
            return;
        }
        let mut values = std::collections::BTreeMap::new();
        for (k, _, _, e) in &fields {
            values.insert(k.clone(), e.text().to_string());
        }
        let Some((backend, params, bucket)) = esp_core::presets::build(preset_id, &values) else {
            return;
        };

        btn.set_sensitive(false);
        btn.set_label("Connecting...");
        status.remove_css_class("error");
        status.remove_css_class("success");
        status.add_css_class("dim");
        status.set_text("Testing the connection\u{2026}");
        status.set_visible(true);
        let (tx, rx) = async_channel::bounded(1);
        let (n, bk) = (name.clone(), bucket.clone());
        std::thread::spawn(move || {
            // Verify against the connected bucket, not the account root: the scoped key we ask users
            // to make cannot list all buckets, so verifying `remote:` would fail a correct key and
            // then delete it. On success, remember the bucket as the remote's landing root.
            let res = esp_core::create_remote(&n, &backend, &params)
                .and_then(|()| {
                    esp_core::verify_remote_path(&n, &bk).inspect_err(|_| {
                        let _ = esp_core::delete_remote(&n); // never keep a remote that does not work
                    })
                })
                .map(|()| {
                    let _ = esp_core::roots::set(&n, &bk);
                });
            let _ = tx.send_blocking(res.map_err(|e| e.to_string()));
        });
        let (ui_c, dlg_c, name_c, btn_c, status_c, bucket_c) = (
            ui2.clone(),
            dlg2.clone(),
            name.clone(),
            btn.clone(),
            status.clone(),
            bucket.clone(),
        );
        glib::spawn_future_local(async move {
            match rx.recv().await {
                Ok(Ok(())) => {
                    // Show the green confirmation inline, then close a beat later so the user sees
                    // their storage was actually reached before the dialog goes away.
                    status_c.remove_css_class("dim");
                    status_c.add_css_class("success");
                    status_c.set_text(&if bucket_c.is_empty() {
                        "Connected.".to_string()
                    } else {
                        format!("Connected. Found your bucket \u{201c}{bucket_c}\u{201d}.")
                    });
                    let (ui_d, dlg_d, name_d) = (ui_c.clone(), dlg_c.clone(), name_c.clone());
                    glib::timeout_add_local_once(
                        std::time::Duration::from_millis(900),
                        move || {
                            dlg_d.close();
                            crate::refresh_all(&ui_d);
                            ui_d.toast(&format!("Added {name_d}"));
                        },
                    );
                }
                Ok(Err(e)) => {
                    btn_c.set_sensitive(true);
                    btn_c.set_label("Connect");
                    status_c.remove_css_class("dim");
                    status_c.add_css_class("error");
                    status_c.set_text(&esp_core::friendly_error(&e));
                }
                Err(_) => {
                    btn_c.set_sensitive(true);
                    btn_c.set_label("Connect");
                    status_c.set_visible(false);
                }
            }
        });
    });

    let scroll = gtk::ScrolledWindow::new();
    scroll.set_vexpand(true);
    scroll.set_child(Some(&prefs));
    let tv = adw::ToolbarView::new();
    tv.add_top_bar(&header);
    tv.set_content(Some(&scroll));
    adw::NavigationPage::builder()
        .title(preset.name)
        .child(&tv)
        .build()
}

fn config_page(
    ui: &Rc<Ui>,
    provider: &esp_core::Provider,
    dialog: &adw::Dialog,
) -> adw::NavigationPage {
    // Internxt connects through rclone's own native backend (verified working with email + password,
    // 2026-11-24), so it uses the standard form like any other provider. The old browser/CLI WebDAV
    // bridge was removed; its source is parked in .dev/parked/ in case a 2FA fallback is ever wanted.
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
                    crate::refresh_all(&ui3);
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

/// Edit an existing remote: the same fields, prefilled from its current config, saved back with
/// `rclone config update`. Password fields start blank (leave blank to keep the current value).
/// Re-authenticate an existing remote whose token or login has gone stale, without removing and
/// re-adding it (which would lose the remote's syncs/backups/mounts). OAuth clouds re-run the
/// browser sign-in in place, refreshing the token; everything else re-opens its credential form so
/// the user can re-enter the password or a fresh 2FA code and save.
pub fn reconnect(ui: &Rc<Ui>, name: &str, backend: &str) {
    if !esp_core::is_oauth_backend(backend) {
        present_edit(ui, name);
        return;
    }
    let port = match ui.ensure_rcd() {
        Ok(r) => r.port(),
        Err(e) => {
            ui.fail("Could not start the engine", &format!("{e}"));
            return;
        }
    };
    ui.toast(&format!("Opening your browser to reconnect {name}\u{2026}"));
    let (tx, rx) = async_channel::bounded(1);
    let (n, b) = (name.to_string(), backend.to_string());
    std::thread::spawn(move || {
        let _ = tx.send_blocking(esp_core::connect::oauth_connect(port, &n, &b));
    });
    let (ui2, name2) = (ui.clone(), name.to_string());
    glib::spawn_future_local(async move {
        match rx.recv().await {
            Ok(Ok(())) => {
                crate::refresh_all(&ui2);
                ui2.toast(&format!("Reconnected {name2}"));
            }
            Ok(Err(e)) => ui2.error(&format!(
                "Could not reconnect: {}",
                esp_core::friendly_error(&e.to_string())
            )),
            Err(_) => {}
        }
    });
}

pub fn present_edit(ui: &Rc<Ui>, name: &str) {
    let values = match esp_core::remote_values(name) {
        Ok(v) => v,
        Err(e) => {
            ui.error(&format!("Could not read \u{201c}{name}\u{201d}: {e}"));
            return;
        }
    };
    let backend = values.get("type").cloned().unwrap_or_default();
    let Some(provider) = ui.providers.iter().find(|p| p.name == backend).cloned() else {
        ui.error(&format!(
            "\u{201c}{name}\u{201d} uses a backend Oortery cannot edit here ({backend})."
        ));
        return;
    };

    let dialog = adw::Dialog::builder()
        .title(format!("Edit {name}"))
        .content_width(560)
        .content_height(640)
        .build();
    let prefs = adw::PreferencesPage::new();
    let header = adw::HeaderBar::new();

    let gopts = adw::PreferencesGroup::builder().title("Settings").build();
    if let Some(hint) = backend_hint(&provider.name) {
        gopts.set_description(Some(hint));
    }
    let mut fields: Vec<(String, gtk::Editable)> = Vec::new();
    let mut add_field = |group: &adw::PreferencesGroup,
                         exp: Option<&adw::ExpanderRow>,
                         o: &esp_core::ProviderOption| {
        let editable: gtk::Editable = if o.is_password || o.sensitive {
            let row = adw::PasswordEntryRow::builder().title(&o.name).build();
            row.set_tooltip_text(Some("Leave blank to keep the current value"));
            match exp {
                Some(e) => e.add_row(&row),
                None => group.add(&row),
            }
            row.upcast()
        } else {
            let row = adw::EntryRow::builder().title(&o.name).build();
            if let Some(v) = values.get(&o.name) {
                row.set_text(v);
            }
            match exp {
                Some(e) => e.add_row(&row),
                None => group.add(&row),
            }
            row.upcast()
        };
        fields.push((o.name.clone(), editable));
    };
    for o in provider.basic_options() {
        if o.name == "token" || o.name == "type" {
            continue;
        }
        add_field(&gopts, None, o);
    }
    prefs.add(&gopts);

    let advanced: Vec<_> = provider
        .options
        .iter()
        .filter(|o| o.advanced && o.hide == 0 && o.name != "token")
        .collect();
    if !advanced.is_empty() {
        let exp = adw::ExpanderRow::builder()
            .title("Advanced settings")
            .build();
        for o in advanced {
            add_field(&gopts, Some(&exp), o);
        }
        let agroup = adw::PreferencesGroup::new();
        agroup.add(&exp);
        prefs.add(&agroup);
    }

    let save = gtk::Button::with_label("Save");
    save.add_css_class("suggested-action");
    header.pack_end(&save);

    let (ui2, dlg2, name2) = (ui.clone(), dialog.clone(), name.to_string());
    save.connect_clicked(move |btn| {
        let params: Vec<(String, String)> = fields
            .iter()
            .map(|(k, e)| (k.clone(), e.text().to_string()))
            .collect();
        btn.set_sensitive(false);
        btn.set_label("Saving...");
        let (tx, rx) = async_channel::bounded(1);
        let (n, p) = (name2.clone(), params);
        std::thread::spawn(move || {
            let _ = tx.send_blocking(esp_core::update_remote(&n, &p).map_err(|e| e.to_string()));
        });
        let (ui_c, dlg_c, name_c, btn_c) = (ui2.clone(), dlg2.clone(), name2.clone(), btn.clone());
        glib::spawn_future_local(async move {
            match rx.recv().await {
                Ok(Ok(())) => {
                    dlg_c.close();
                    crate::refresh_all(&ui_c);
                    ui_c.toast(&format!("Saved {name_c}"));
                }
                Ok(Err(e)) => {
                    btn_c.set_sensitive(true);
                    btn_c.set_label("Save");
                    ui_c.error(&format!("Could not save: {e}"));
                }
                Err(_) => {
                    btn_c.set_sensitive(true);
                    btn_c.set_label("Save");
                }
            }
        });
    });

    let scroll = gtk::ScrolledWindow::new();
    scroll.set_vexpand(true);
    scroll.set_child(Some(&prefs));
    let tv = adw::ToolbarView::new();
    tv.add_top_bar(&header);
    tv.set_content(Some(&scroll));
    dialog.set_child(Some(&tv));
    dialog.present(Some(&ui.window));
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
    // A plain-language note for backends with a known gotcha, so a new user is not left guessing.
    if let Some(hint) = backend_hint(&provider.name) {
        gopts.set_description(Some(hint));
    }
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

    // Advanced settings: options some accounts need (e.g. Proton app_version / mailbox_password)
    // that the common case does not, tucked into an expander so they do not clutter the form.
    let advanced: Vec<_> = provider
        .options
        .iter()
        .filter(|o| o.advanced && o.hide == 0 && o.name != "token")
        .collect();
    if !advanced.is_empty() {
        let exp = adw::ExpanderRow::builder()
            .title("Advanced settings")
            .subtitle("Only needed for some accounts")
            .build();
        for o in advanced {
            let editable: gtk::Editable = if o.is_password || o.sensitive {
                let row = adw::PasswordEntryRow::builder().title(&o.name).build();
                exp.add_row(&row);
                row.upcast()
            } else {
                let row = adw::EntryRow::builder().title(&o.name).build();
                exp.add_row(&row);
                row.upcast()
            };
            fields.push((o.name.clone(), false, editable));
        }
        let agroup = adw::PreferencesGroup::new();
        agroup.add(&exp);
        prefs.add(&agroup);
    }

    let create = gtk::Button::with_label("Create");
    create.add_css_class("suggested-action");
    header.pack_end(&create);

    let ui2 = ui.clone();
    let dlg2 = dialog.clone();
    let backend = provider.name.clone();
    // The rclone fields that are passwords (IsPassword, not merely sensitive-display). rclone stores
    // these obscured and reveals them at use time; some backends (Internxt) reveal during config
    // create, so they must be obscured before create or the login reads garbage and 401s.
    let pw_fields: std::collections::HashSet<String> = provider
        .options
        .iter()
        .filter(|o| o.is_password)
        .map(|o| o.name.clone())
        .collect();
    let name_row2 = name_row.clone();
    create.connect_clicked(move |btn| {
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

        // Some backends (Proton, Internxt, Mega, ...) do a real network login at create time, which
        // can take several seconds. Run it off the UI thread so the window never freezes; the button
        // shows a connecting state and re-enables on failure.
        btn.set_sensitive(false);
        btn.set_label("Connecting...");

        // A few backends (Internxt) only write their encryption material in a later state of
        // rclone's config flow, so a one-shot `create` leaves a broken, mnemonic-less remote. Those
        // go through the state machine, which needs the rc daemon running. Everyone else keeps the
        // simpler one-shot create.
        let needs_flow = esp_core::connect::needs_login_flow(&backend);
        let port = if needs_flow {
            match ui2.ensure_rcd() {
                Ok(r) => Some(r.port()),
                Err(e) => {
                    btn.set_sensitive(true);
                    btn.set_label("Create");
                    ui2.error(&format!("Could not start the engine: {e}"));
                    return;
                }
            }
        } else {
            None
        };

        let (tx, rx) = async_channel::bounded(1);
        let (n, b) = (name.clone(), backend.clone());
        let pw = pw_fields.clone();
        std::thread::spawn(move || {
            // Obscure password fields before create. rclone reveals these at use time, and Internxt
            // reveals during config create itself, so a plaintext password is read as garbage and the
            // backend returns a bogus "Wrong login credentials (401)". Obscuring here matches the
            // verified-working `rclone config create ... pass $(rclone obscure PW)`.
            let prepared: anyhow::Result<Vec<(String, String)>> = params
                .into_iter()
                .map(|(k, v)| {
                    if !v.is_empty() && pw.contains(&k) {
                        esp_core::obscure(&v).map(|o| (k, o))
                    } else {
                        Ok((k, v))
                    }
                })
                .collect();
            let res = prepared.and_then(|params| {
                match port {
                    Some(p) => esp_core::connect::credential_connect(p, &n, &b, &params),
                    None => esp_core::create_remote(&n, &b, &params),
                }
                .and_then(|()| {
                    esp_core::verify_remote(&n).inspect_err(|_| {
                        let _ = esp_core::delete_remote(&n); // never keep a remote that does not work
                    })
                })
            });
            // Format the whole error chain ({:#}) so the user sees rclone's real cause (e.g.
            // "Wrong login credentials"), not just our top-level "starting config/create" context.
            let _ = tx.send_blocking(res.map_err(|e| format!("{e:#}")));
        });
        let (ui_c, dlg_c, btn_c, name_c) = (ui2.clone(), dlg2.clone(), btn.clone(), name.clone());
        glib::spawn_future_local(async move {
            match rx.recv().await {
                Ok(Ok(())) => {
                    dlg_c.close();
                    crate::refresh_all(&ui_c);
                    ui_c.toast(&format!("Added {name_c}"));
                }
                Ok(Err(e)) => {
                    btn_c.set_sensitive(true);
                    btn_c.set_label("Create");
                    ui_c.error(&format!("Could not add: {}", esp_core::friendly_error(&e)));
                }
                Err(_) => {
                    btn_c.set_sensitive(true);
                    btn_c.set_label("Create");
                }
            }
        });
    });
}
