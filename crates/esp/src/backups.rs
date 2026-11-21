//! The Backups page: versioned, encrypted restic snapshots to a local folder or any cloud.
//! Running a backup and restoring both happen on worker threads (restic can take a while and
//! touch the network), so the UI shows progress instead of freezing.

use crate::{folder_browser, syncs, Ui};
use adw::prelude::*;
use esp_core::BackupJob;
use gtk::glib;
use std::rc::Rc;

/// Messages from a running backup worker thread.
enum Msg {
    Progress(f64),
    Verifying,
    Done(Result<String, String>),
}

/// Build the Backups page. Returns the page, the job list, and the "New backup" button.
pub fn page() -> (gtk::Widget, gtk::ListBox, gtk::Button) {
    let clamp = adw::Clamp::new();
    clamp.set_maximum_size(820);
    let scroll = gtk::ScrolledWindow::new();
    scroll.set_vexpand(true);
    let col = gtk::Box::new(gtk::Orientation::Vertical, 10);
    col.set_margin_top(24);
    col.set_margin_bottom(24);
    col.set_margin_start(24);
    col.set_margin_end(24);

    let header = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let titles = gtk::Box::new(gtk::Orientation::Vertical, 2);
    titles.set_hexpand(true);
    let heading = gtk::Label::new(Some("Backups"));
    heading.add_css_class("title-4");
    heading.set_xalign(0.0);
    let sub = gtk::Label::new(Some(
        "Versioned, encrypted snapshots you can restore from. Local or any cloud.",
    ));
    sub.add_css_class("dim");
    sub.set_xalign(0.0);
    titles.append(&heading);
    titles.append(&sub);
    let new_btn = gtk::Button::builder()
        .label("New backup")
        .valign(gtk::Align::Center)
        .build();
    new_btn.add_css_class("suggested-action");
    header.append(&titles);
    header.append(&new_btn);
    header.set_margin_bottom(6);

    let list = gtk::ListBox::new();
    list.add_css_class("boxed-list");
    list.set_selection_mode(gtk::SelectionMode::None);

    col.append(&header);
    col.append(&list);
    scroll.set_child(Some(&col));
    clamp.set_child(Some(&scroll));
    (clamp.upcast(), list, new_btn)
}

/// Rebuild the backups list from the saved jobs.
pub fn refresh(ui: &Rc<Ui>) {
    while let Some(child) = ui.backups_list.first_child() {
        ui.backups_list.remove(&child);
    }
    let jobs = esp_core::load_backups().unwrap_or_default();
    ui.backups_count.set_text(&jobs.len().to_string());
    if esp_core::restic_path().is_none() {
        let row = adw::ActionRow::builder()
            .title("restic is not installed")
            .subtitle("Backups need the restic tool on your system. Install it, then restart ESP.")
            .build();
        row.add_prefix(&gtk::Image::from_icon_name("dialog-warning-symbolic"));
        ui.backups_list.append(&row);
        return;
    }
    if jobs.is_empty() {
        let row = adw::ActionRow::builder()
            .title("No backups yet")
            .subtitle("Create a backup to keep versioned, restorable history.")
            .build();
        row.add_prefix(&gtk::Image::from_icon_name("drive-harddisk-symbolic"));
        ui.backups_list.append(&row);
        return;
    }
    for job in jobs {
        ui.backups_list.append(&job_row(ui, job));
    }

    // Headless aid: drive the first backup's Run through the real path once mapped.
    if std::env::var("ESP_BACKUP_AUTORUN").is_ok() {
        if let Some(first) = ui.backups_list.first_child() {
            if let Some(btn) = syncs::find_run_button(&first) {
                gtk::glib::timeout_add_local_once(
                    std::time::Duration::from_millis(800),
                    move || {
                        btn.activate();
                    },
                );
            }
        }
    }
}

fn job_row(ui: &Rc<Ui>, job: BackupJob) -> adw::ActionRow {
    let repo = job.repo.display();
    let src = job.sources.first().cloned().unwrap_or_default();
    let base_subtitle = format!("{src} to {repo}");
    let row = adw::ActionRow::builder()
        .title(&job.name)
        .subtitle(&base_subtitle)
        .build();
    row.add_prefix(&gtk::Image::from_icon_name("drive-harddisk-symbolic"));

    let bar = gtk::ProgressBar::new();
    bar.set_valign(gtk::Align::Center);
    bar.set_width_request(150);
    bar.set_visible(false);
    row.add_suffix(&bar);

    let snaps = gtk::Button::from_icon_name("document-open-recent-symbolic");
    snaps.set_tooltip_text(Some("Snapshots and restore"));
    snaps.set_valign(gtk::Align::Center);
    snaps.add_css_class("flat");
    row.add_suffix(&snaps);

    let run = gtk::Button::from_icon_name("media-playback-start-symbolic");
    run.set_tooltip_text(Some("Back up now"));
    run.set_valign(gtk::Align::Center);
    run.add_css_class("flat");
    row.add_suffix(&run);

    let (ui_s, job_s) = (ui.clone(), job.clone());
    snaps.connect_clicked(move |_| present_snapshots(&ui_s, &job_s));

    // Edit / Delete menu. Delete also removes the repo's password file (not the snapshots).
    let (menu, edit_btn, del_btn, pop) = syncs::row_menu();
    row.add_suffix(&menu);
    {
        let (ui_e, job_e, pop_e) = (ui.clone(), job.clone(), pop.clone());
        edit_btn.connect_clicked(move |_| {
            pop_e.popdown();
            present_new(&ui_e, Some(job_e.clone()));
        });
        let (ui_d, pop_d, id, nm) = (ui.clone(), pop.clone(), job.id.clone(), job.name.clone());
        del_btn.connect_clicked(move |_| {
            pop_d.popdown();
            syncs::confirm_delete(&ui_d, &nm, "backup", {
                let (ui2, id2) = (ui_d.clone(), id.clone());
                move || {
                    let mut jobs = esp_core::load_backups().unwrap_or_default();
                    jobs.retain(|j| j.id != id2);
                    let _ = esp_core::save_backups(&jobs);
                    if let Ok(pw) = esp_core::password_file(&id2) {
                        let _ = std::fs::remove_file(pw);
                    }
                    refresh(&ui2);
                }
            });
        });
    }

    let ui_r = ui.clone();
    let row_r = row.clone();
    let bar_r = bar.clone();
    let run_r = run.clone();
    run.connect_clicked(move |_| run_backup(&ui_r, &job, &base_subtitle, &row_r, &bar_r, &run_r));
    row
}

/// Run a backup on a worker thread, streaming restic's progress into the row.
fn run_backup(
    ui: &Rc<Ui>,
    job: &BackupJob,
    base_subtitle: &str,
    row: &adw::ActionRow,
    bar: &gtk::ProgressBar,
    run: &gtk::Button,
) {
    run.set_sensitive(false);
    bar.set_visible(true);
    bar.set_fraction(0.0);
    row.set_subtitle("Backing up...");

    let (tx, rx) = async_channel::unbounded::<Msg>();
    let job_t = job.clone();
    std::thread::spawn(move || {
        let txp = tx.clone();
        let res = esp_core::init_repo(&job_t).and_then(|()| {
            esp_core::run_backup(&job_t, |phase| {
                let _ = txp.send_blocking(match phase {
                    esp_core::BackupPhase::Backing(p) => Msg::Progress(p),
                    esp_core::BackupPhase::Verifying => Msg::Verifying,
                });
            })
        });
        let _ = tx.send_blocking(Msg::Done(res.map_err(|e| e.to_string())));
    });

    let (ui_p, row_p, bar_p, run_p) = (ui.clone(), row.clone(), bar.clone(), run.clone());
    let base = base_subtitle.to_string();
    let name = job.name.clone();
    glib::spawn_future_local(async move {
        while let Ok(msg) = rx.recv().await {
            match msg {
                Msg::Progress(p) => bar_p.set_fraction(p),
                Msg::Verifying => {
                    // Honest state: the backup is written but not yet trustworthy. Show the
                    // integrity step rather than a premature 100%.
                    bar_p.set_fraction(1.0);
                    row_p.set_subtitle("Verifying integrity...");
                }
                Msg::Done(res) => {
                    run_p.set_sensitive(true);
                    bar_p.set_visible(false);
                    row_p.set_subtitle(&base);
                    match res {
                        Ok(_) => ui_p.toast(&format!("{name} backed up")),
                        Err(e) => ui_p.toast(&format!("{name} backup failed: {e}")),
                    }
                    break;
                }
            }
        }
    });
}

/// A dialog listing the repo's snapshots, each with a Restore action.
fn present_snapshots(ui: &Rc<Ui>, job: &BackupJob) {
    let dialog = adw::Dialog::builder()
        .title("Snapshots")
        .content_width(520)
        .content_height(520)
        .build();
    let list = gtk::ListBox::new();
    list.add_css_class("boxed-list");
    list.set_selection_mode(gtk::SelectionMode::None);
    let spinner = gtk::Spinner::new();
    spinner.set_halign(gtk::Align::Center);
    spinner.set_margin_top(16);
    spinner.start();
    let scroll = gtk::ScrolledWindow::new();
    scroll.set_vexpand(true);
    scroll.set_child(Some(&list));
    let body = gtk::Box::new(gtk::Orientation::Vertical, 8);
    body.set_margin_top(10);
    body.set_margin_bottom(12);
    body.set_margin_start(12);
    body.set_margin_end(12);
    body.append(&spinner);
    body.append(&scroll);
    let tv = adw::ToolbarView::new();
    tv.add_top_bar(&adw::HeaderBar::new());
    tv.set_content(Some(&body));
    dialog.set_child(Some(&tv));

    // Load snapshots off the UI thread (listing a cloud repo hits the network).
    let (tx, rx) = async_channel::bounded(1);
    let job_t = job.clone();
    std::thread::spawn(move || {
        let _ = tx.send_blocking(esp_core::list_snapshots(&job_t).map_err(|e| e.to_string()));
    });
    let (ui_c, job_c, list_c, spinner_c) = (ui.clone(), job.clone(), list.clone(), spinner.clone());
    glib::spawn_future_local(async move {
        let res = rx.recv().await;
        spinner_c.stop();
        spinner_c.set_visible(false);
        match res {
            Ok(Ok(snaps)) if !snaps.is_empty() => {
                for s in snaps {
                    let row = adw::ActionRow::builder()
                        .title(s.time.get(..19).unwrap_or(&s.time))
                        .subtitle(&s.short_id)
                        .build();
                    row.add_prefix(&gtk::Image::from_icon_name("document-open-recent-symbolic"));
                    let restore = gtk::Button::with_label("Restore");
                    restore.set_valign(gtk::Align::Center);
                    let (ui_b, job_b, id) = (ui_c.clone(), job_c.clone(), s.id.clone());
                    restore.connect_clicked(move |_| restore_snapshot(&ui_b, &job_b, &id));
                    row.add_suffix(&restore);
                    list_c.append(&row);
                }
            }
            Ok(Ok(_)) => {
                let row = adw::ActionRow::builder()
                    .title("No snapshots yet")
                    .subtitle("Run this backup to create the first one.")
                    .build();
                list_c.append(&row);
            }
            Ok(Err(e)) => ui_c.toast(&format!("Could not list snapshots: {e}")),
            Err(_) => {}
        }
    });

    dialog.present(Some(&ui.window));
}

/// Pick a target folder, then restore the snapshot into it on a worker thread.
fn restore_snapshot(ui: &Rc<Ui>, job: &BackupJob, snapshot_id: &str) {
    let (ui2, job2, id) = (ui.clone(), job.clone(), snapshot_id.to_string());
    let on_pick: Rc<dyn Fn(String)> = Rc::new(move |sub: String| {
        let target = format!("/{sub}");
        let (tx, rx) = async_channel::bounded(1);
        let (job_t, id_t, target_t) = (job2.clone(), id.clone(), target.clone());
        std::thread::spawn(move || {
            let _ = tx.send_blocking(
                esp_core::restore(&job_t, &id_t, &target_t).map_err(|e| e.to_string()),
            );
        });
        let (ui_c, target_c) = (ui2.clone(), target.clone());
        glib::spawn_future_local(async move {
            match rx.recv().await {
                Ok(Ok(())) => ui_c.toast(&format!("Restored to {target_c}")),
                Ok(Err(e)) => ui_c.toast(&format!("Restore failed: {e}")),
                Err(_) => {}
            }
        });
    });
    folder_browser::present(ui, "/".to_string(), String::new(), on_pick);
}

/// The new-backup dialog: name, a source folder, a repo location, and a password.
pub fn present_new(ui: &Rc<Ui>, edit: Option<BackupJob>) {
    let remotes = esp_core::list_remotes().unwrap_or_default();
    let mut options: Vec<Option<String>> = vec![None];
    let mut labels: Vec<String> = vec!["This computer".to_string()];
    for r in &remotes {
        options.push(Some(r.name.clone()));
        labels.push(format!(
            "{} ({})",
            r.name,
            esp_core::backend_label(&r.backend)
        ));
    }
    let label_refs: Vec<&str> = labels.iter().map(|s| s.as_str()).collect();

    let dialog = adw::Dialog::builder()
        .title(if edit.is_some() {
            "Edit backup"
        } else {
            "New backup"
        })
        .content_width(560)
        .content_height(640)
        .build();
    let prefs = adw::PreferencesPage::new();

    let gname = adw::PreferencesGroup::builder().title("Backup").build();
    let name_row = adw::EntryRow::builder().title("Name").build();
    gname.add(&name_row);
    prefs.add(&gname);

    // Source: a local folder to protect.
    let gsrc = adw::PreferencesGroup::builder()
        .title("Source")
        .description("The folder on this computer to back up.")
        .build();
    let src_path = adw::EntryRow::builder().title("Folder").build();
    let src_browse = gtk::Button::from_icon_name("folder-open-symbolic");
    src_browse.set_tooltip_text(Some("Browse folders"));
    src_browse.set_valign(gtk::Align::Center);
    src_browse.add_css_class("flat");
    src_path.add_suffix(&src_browse);
    gsrc.add(&src_path);
    prefs.add(&gsrc);
    let (ui_sb, src_path_b) = (ui.clone(), src_path.clone());
    src_browse.connect_clicked(move |_| {
        let cur = src_path_b.text().to_string();
        let start = cur.trim_start_matches('/').to_string();
        let p = src_path_b.clone();
        let on_pick: Rc<dyn Fn(String)> = Rc::new(move |sub| p.set_text(&format!("/{sub}")));
        folder_browser::present(&ui_sb, "/".to_string(), start, on_pick);
    });

    // Repo: where the encrypted snapshots live (local or a cloud).
    let grepo = adw::PreferencesGroup::builder()
        .title("Repository")
        .description("Where the encrypted snapshots are stored.")
        .build();
    let (repo_combo, repo_path, repo_browse) = syncs::endpoint_rows(&label_refs);
    grepo.add(&repo_combo);
    grepo.add(&repo_path);
    prefs.add(&grepo);
    syncs::wire_browse(ui, options.clone(), &repo_combo, &repo_path, &repo_browse);

    let gpw = adw::PreferencesGroup::builder()
        .title("Password")
        .description(
            "Encrypts the repository. Keep it safe: without it the backups cannot be read.",
        )
        .build();
    let pw_row = adw::PasswordEntryRow::builder().title("Password").build();
    gpw.add(&pw_row);
    prefs.add(&gpw);

    // Prefill when editing an existing backup (the password stays in its file, not shown).
    if let Some(j) = &edit {
        name_row.set_text(&j.name);
        src_path.set_text(j.sources.first().map(|s| s.as_str()).unwrap_or(""));
        repo_combo.set_selected(syncs::combo_index_for(&options, &j.repo));
        repo_path.set_text(&j.repo.path);
        pw_row.set_title("Password (leave blank to keep the current one)");
    }

    let scroll = gtk::ScrolledWindow::new();
    scroll.set_vexpand(true);
    scroll.set_child(Some(&prefs));
    let header = adw::HeaderBar::new();
    let create = gtk::Button::with_label(if edit.is_some() { "Save" } else { "Create" });
    create.add_css_class("suggested-action");
    header.pack_end(&create);
    let tv = adw::ToolbarView::new();
    tv.add_top_bar(&header);
    tv.set_content(Some(&scroll));
    dialog.set_child(Some(&tv));

    let ui2 = ui.clone();
    let dlg2 = dialog.clone();
    let edit_id = edit.as_ref().map(|j| j.id.clone());
    create.connect_clicked(move |_| {
        let name = name_row.text().trim().to_string();
        if name.is_empty() {
            ui2.toast("Give the backup a name.");
            return;
        }
        let source = src_path.text().trim().to_string();
        if source.is_empty() {
            ui2.toast("Choose a source folder.");
            return;
        }
        let password = pw_row.text().to_string();
        if password.is_empty() && edit_id.is_none() {
            ui2.toast("Set a password to encrypt the repository.");
            return;
        }
        let repo = syncs::endpoint_from(&options, repo_combo.selected(), &repo_path.text());
        if repo.remote.is_none() && repo.path.is_empty() {
            ui2.toast("Choose where the repository lives.");
            return;
        }

        let mut jobs = esp_core::load_backups().unwrap_or_default();
        let id = match &edit_id {
            Some(id) => id.clone(),
            None => {
                let base = esp_core::slug_id(&name, &[]);
                let mut id = base.clone();
                let mut n = 2;
                while jobs.iter().any(|j| j.id == id) {
                    id = format!("{base}-{n}");
                    n += 1;
                }
                id
            }
        };
        // Only (re)write the password when one was entered; blank on edit keeps the current.
        if !password.is_empty() {
            if let Err(e) = esp_core::save_password(&id, &password) {
                ui2.toast(&format!("Could not save the password: {e}"));
                return;
            }
        }
        let new_job = BackupJob {
            id: id.clone(),
            name: name.clone(),
            sources: vec![source],
            repo,
        };
        match jobs.iter().position(|j| j.id == id) {
            Some(pos) => jobs[pos] = new_job,
            None => jobs.push(new_job),
        }
        match esp_core::save_backups(&jobs) {
            Ok(()) => {
                dlg2.close();
                refresh(&ui2);
                ui2.toast(&format!(
                    "Saved {name}. Back it up to create the first snapshot."
                ));
            }
            Err(e) => ui2.toast(&format!("Could not save: {e}")),
        }
    });

    dialog.present(Some(&ui.window));
}
