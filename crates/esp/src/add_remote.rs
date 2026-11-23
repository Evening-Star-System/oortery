//! The add-remote flow: pick any backend rclone supports, then connect it. OAuth clouds
//! (Drive, Dropbox, OneDrive, ...) sign in with one tap in the browser; config-value
//! backends (S3, SFTP, WebDAV, B2, and many more) fill a short form. Both end up connected.

use crate::Ui;
use adw::prelude::*;
use gtk::glib;
use std::cell::RefCell;
use std::rc::Rc;

/// The top providers shown under "Popular", by real-world popularity: the three big consumer clouds,
/// a cheap object store, and a privacy-first cloud. A key is matched to an easy preset first (B2),
/// else to the rclone provider of the same name (which decides OAuth vs form). Easy to re-order.
const POPULAR: &[&str] = &["drive", "dropbox", "onedrive", "b2", "pcloud"];

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

/// A re-render hook for the Internxt wizard: an action's success calls it to advance the step.
type RenderHook = Rc<dyn Fn()>;

/// A plain-language note for backends with a known gotcha, shown above the form so a new user is
/// not left guessing. (Internxt has no entry here: it routes to its own guided helper, not this
/// form, because its native login is dead.)
fn backend_hint(backend: &str) -> Option<&'static str> {
    match backend {
        "protondrive" => Some(
            "If you use two-factor authentication, put your current 6-digit code in the 2fa field. \
             A separate mailbox password (two-password accounts) goes under Advanced settings. If a \
             sign-in is refused right after several tries, wait a few minutes: Proton temporarily \
             blocks repeated attempts.",
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

    // The handful of providers most people actually reach for, offered first so the long list is
    // optional. OAuth clouds route to one-tap sign-in; the object store (B2) uses its easy preset
    // form. Ordered by real-world popularity; trivial to re-order or extend.
    let popular = gtk::Label::new(Some("Popular"));
    popular.set_xalign(0.0);
    popular.add_css_class("heading");
    let presets_list = gtk::ListBox::new();
    presets_list.add_css_class("boxed-list");
    presets_list.set_selection_mode(gtk::SelectionMode::None);
    for key in POPULAR {
        // Prefer a stupid-easy preset if one exists for this key (e.g. B2); otherwise the provider.
        if let Some(preset) = esp_core::presets::preset(key) {
            let row = adw::ActionRow::builder()
                .title(preset.name)
                .subtitle(preset.blurb)
                .activatable(true)
                .build();
            row.add_prefix(&gtk::Image::from_icon_name("folder-remote-symbolic"));
            row.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
            let (ui2, nav2, dlg2) = (ui.clone(), nav.clone(), dialog.clone());
            row.connect_activated(move |_| {
                nav2.push(&preset_page(&ui2, preset, &dlg2));
            });
            presets_list.append(&row);
        } else if let Some(p) = ui.providers.iter().find(|p| &p.name == key).cloned() {
            let subtitle = if p.needs_oauth() {
                "Sign in with your browser"
            } else {
                "Fill in a short form"
            };
            let row = adw::ActionRow::builder()
                .title(first_line(&p.description))
                .subtitle(subtitle)
                .activatable(true)
                .build();
            row.add_prefix(&gtk::Image::from_icon_name("folder-remote-symbolic"));
            row.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
            let (ui2, nav2, dlg2) = (ui.clone(), nav.clone(), dialog.clone());
            row.connect_activated(move |_| {
                nav2.push(&config_page(&ui2, &p, &dlg2));
            });
            presets_list.append(&row);
        }
    }
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
    body.append(&all_label);
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

    let create = gtk::Button::with_label("Create");
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
        let (ui_c, dlg_c, name_c, btn_c) = (ui2.clone(), dlg2.clone(), name.clone(), btn.clone());
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

/// Internxt helper: a small wizard over Internxt's own CLI WebDAV bridge. Internxt moved to
/// browser login, so the native backend 401s; here we (optionally install the CLI on demand, then)
/// sign in through the browser and stand up a local bridge, ending with a working WebDAV remote.
/// Each CLI call runs off the UI thread (Node is slow to start; blocking here would freeze the app,
/// the same failure mode we fixed for mounts).
fn internxt_page(ui: &Rc<Ui>, dialog: &adw::Dialog) -> adw::NavigationPage {
    let prefs = adw::PreferencesPage::new();

    let intro = adw::PreferencesGroup::builder()
        .title("Connect Internxt")
        .description(
            "Internxt now signs you in through your browser, so Oortery connects through \
             Internxt's own secure bridge. Your files stay end-to-end encrypted: the bridge does \
             the crypto on your machine, and Oortery never sees your password.",
        )
        .build();
    prefs.add(&intro);

    let gname = adw::PreferencesGroup::new();
    let name_row = adw::EntryRow::builder().title("Name").build();
    name_row.set_text("Internxt");
    gname.add(&name_row);
    prefs.add(&gname);

    // The step area is rebuilt from the live readiness after each action, so the wizard advances
    // install -> sign in -> connect without the user navigating anywhere.
    let step_group = adw::PreferencesGroup::new();
    let step_box = gtk::Box::new(gtk::Orientation::Vertical, 10);
    step_box.set_margin_top(4);
    step_group.add(&step_box);
    prefs.add(&step_group);

    // A self-referencing render closure: an action's success calls it again to show the next step.
    let render_cell: Rc<RefCell<Option<RenderHook>>> = Rc::new(RefCell::new(None));
    let render: RenderHook = {
        let (ui, dialog, name_row, step_box, render_cell) = (
            ui.clone(),
            dialog.clone(),
            name_row.clone(),
            step_box.clone(),
            render_cell.clone(),
        );
        Rc::new(move || {
            // Show an immediate "checking" line, then read readiness off-thread.
            while let Some(c) = step_box.first_child() {
                step_box.remove(&c);
            }
            let checking = gtk::Label::new(Some("Checking Internxt\u{2026}"));
            checking.add_css_class("dim");
            checking.set_xalign(0.0);
            step_box.append(&checking);

            let (tx, rx) = async_channel::bounded(1);
            std::thread::spawn(move || {
                let _ = tx.send_blocking(esp_core::internxt::readiness());
            });

            let (ui, dialog, name_row, step_box, render_cell) = (
                ui.clone(),
                dialog.clone(),
                name_row.clone(),
                step_box.clone(),
                render_cell.clone(),
            );
            glib::spawn_future_local(async move {
                let Ok(readiness) = rx.recv().await else {
                    return;
                };
                while let Some(c) = step_box.first_child() {
                    step_box.remove(&c);
                }
                let again = render_cell.borrow().clone();
                internxt_step(&ui, &dialog, &name_row, &step_box, readiness, again);
            });
        })
    };
    *render_cell.borrow_mut() = Some(render.clone());
    render();

    let header = adw::HeaderBar::new();
    let scroll = gtk::ScrolledWindow::new();
    scroll.set_vexpand(true);
    scroll.set_child(Some(&prefs));
    let tv = adw::ToolbarView::new();
    tv.add_top_bar(&header);
    tv.set_content(Some(&scroll));
    adw::NavigationPage::builder()
        .title("Internxt")
        .child(&tv)
        .build()
}

/// Populate the Internxt wizard's step area for the current `readiness`. `again` re-renders the
/// step after an action succeeds, advancing the wizard.
fn internxt_step(
    ui: &Rc<Ui>,
    dialog: &adw::Dialog,
    name_row: &adw::EntryRow,
    step_box: &gtk::Box,
    readiness: esp_core::internxt::Readiness,
    again: Option<RenderHook>,
) {
    use esp_core::internxt::Readiness;

    // A labelled note + optional primary button, the shape every step uses.
    let note = |text: &str| {
        let l = gtk::Label::new(Some(text));
        l.add_css_class("dim");
        l.set_xalign(0.0);
        l.set_wrap(true);
        l
    };

    match readiness {
        Readiness::NeedsCli { node: false } => {
            step_box.append(&note(
                "Connecting Internxt needs Node.js, a small free runtime for its helper. Install it, \
                 then press Check again and Oortery handles the rest. (Most Oortery builds include \
                 this already, so you should rarely see this screen.)",
            ));
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            let get = gtk::Button::with_label("Get Node.js");
            get.add_css_class("pill");
            get.connect_clicked(|_| {
                let _ = gtk::gio::AppInfo::launch_default_for_uri(
                    "https://nodejs.org/en/download",
                    None::<&gtk::gio::AppLaunchContext>,
                );
            });
            let recheck = gtk::Button::with_label("Check again");
            recheck.add_css_class("suggested-action");
            recheck.add_css_class("pill");
            let again_r = again.clone();
            recheck.connect_clicked(move |_| {
                if let Some(r) = &again_r {
                    r();
                }
            });
            row.append(&get);
            row.append(&recheck);
            step_box.append(&row);
        }
        Readiness::NeedsCli { node: true } => {
            step_box.append(&note(
                "Oortery will install Internxt's small helper (about 180 MB, one time). Only this \
                 step needs it; the rest is automatic.",
            ));
            let btn = gtk::Button::with_label("Install Internxt support");
            btn.add_css_class("suggested-action");
            btn.add_css_class("pill");
            let (ui, again) = (ui.clone(), again.clone());
            btn.connect_clicked(move |b| {
                b.set_sensitive(false);
                b.set_label("Installing\u{2026}");
                ui.toast("Installing Internxt support\u{2026}");
                let (tx, rx) = async_channel::bounded(1);
                std::thread::spawn(move || {
                    let _ = tx.send_blocking(
                        esp_core::internxt::install_cli().map_err(|e| e.to_string()),
                    );
                });
                let (ui2, b2, again2) = (ui.clone(), b.clone(), again.clone());
                glib::spawn_future_local(async move {
                    match rx.recv().await {
                        Ok(Ok(())) => {
                            ui2.toast("Internxt support installed");
                            if let Some(r) = &again2 {
                                r();
                            }
                        }
                        Ok(Err(e)) => {
                            b2.set_sensitive(true);
                            b2.set_label("Install Internxt support");
                            ui2.error(&format!("Could not install Internxt support: {e}"));
                        }
                        Err(_) => {}
                    }
                });
            });
            step_box.append(&btn);
        }
        Readiness::NeedsLogin => {
            step_box.append(&note(
                "Sign in to Internxt in your browser. Oortery never sees your password; Internxt \
                 hands back only a session on your own machine.",
            ));
            let btn = gtk::Button::with_label("Sign in to Internxt");
            btn.add_css_class("suggested-action");
            btn.add_css_class("pill");
            let (ui, again) = (ui.clone(), again.clone());
            btn.connect_clicked(move |b| {
                b.set_sensitive(false);
                b.set_label("Waiting for sign-in\u{2026}");
                ui.toast("Opening your browser to sign in to Internxt\u{2026}");
                let (tx, rx) = async_channel::bounded(1);
                std::thread::spawn(move || {
                    let _ =
                        tx.send_blocking(esp_core::internxt::login().map_err(|e| e.to_string()));
                });
                let (ui2, b2, again2) = (ui.clone(), b.clone(), again.clone());
                glib::spawn_future_local(async move {
                    match rx.recv().await {
                        Ok(Ok(())) => {
                            ui2.toast("Signed in to Internxt");
                            if let Some(r) = &again2 {
                                r();
                            }
                        }
                        Ok(Err(e)) => {
                            b2.set_sensitive(true);
                            b2.set_label("Sign in to Internxt");
                            ui2.error(&format!("Sign-in did not finish: {e}"));
                        }
                        Err(_) => {}
                    }
                });
            });
            step_box.append(&btn);
        }
        Readiness::Ready => {
            step_box.append(&note(
                "Signed in. Oortery will start your private Internxt bridge and connect it.",
            ));
            let btn = gtk::Button::with_label("Connect Internxt");
            btn.add_css_class("suggested-action");
            btn.add_css_class("pill");
            let (ui, dialog, name_row) = (ui.clone(), dialog.clone(), name_row.clone());
            btn.connect_clicked(move |b| {
                let name = name_row.text().trim().to_string();
                if name.is_empty() {
                    ui.toast("Give this remote a name first.");
                    return;
                }
                b.set_sensitive(false);
                b.set_label("Connecting\u{2026}");
                ui.toast("Starting your Internxt bridge\u{2026}");
                let (tx, rx) = async_channel::bounded(1);
                let n = name.clone();
                std::thread::spawn(move || {
                    let _ = tx
                        .send_blocking(esp_core::internxt::connect(&n).map_err(|e| e.to_string()));
                });
                let (ui2, dlg2, b2) = (ui.clone(), dialog.clone(), b.clone());
                glib::spawn_future_local(async move {
                    match rx.recv().await {
                        Ok(Ok(())) => {
                            dlg2.close();
                            crate::refresh_all(&ui2);
                            ui2.toast(&format!("Connected to Internxt as {name}"));
                        }
                        Ok(Err(e)) => {
                            b2.set_sensitive(true);
                            b2.set_label("Connect Internxt");
                            ui2.error(&format!("Could not connect Internxt: {e}"));
                        }
                        Err(_) => {}
                    }
                });
            });
            step_box.append(&btn);
        }
    }
}

fn config_page(
    ui: &Rc<Ui>,
    provider: &esp_core::Provider,
    dialog: &adw::Dialog,
) -> adw::NavigationPage {
    // Internxt's native login is dead (they moved to browser auth), so route it to a guided helper
    // that connects through Internxt's own CLI WebDAV bridge instead of the 401-guaranteed form.
    if provider.name == "internxt" {
        return internxt_page(ui, dialog);
    }
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
        std::thread::spawn(move || {
            let created = match port {
                Some(p) => esp_core::connect::credential_connect(p, &n, &b, &params),
                None => esp_core::create_remote(&n, &b, &params),
            };
            let res = created.and_then(|()| {
                esp_core::verify_remote(&n).inspect_err(|_| {
                    let _ = esp_core::delete_remote(&n); // never keep a remote that does not work
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
