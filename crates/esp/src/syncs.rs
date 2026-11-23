//! The Syncs page: list saved jobs, create new ones, and run them with live progress.
//!
//! Jobs persist via esp-core; running drives the shared rcd daemon. Progress is polled on
//! the GTK main loop (one fast local `rclone rc` call per tick), which is plenty for the
//! handful of jobs this page shows; it moves to a worker thread only if that ever hitches.

use crate::Ui;
use adw::prelude::*;
use esp_core::{Endpoint, Job, JobStats, SyncMode};
use gtk::glib;
use std::rc::Rc;
use std::time::Duration;

/// Build the Syncs page. Returns the page, the job list to refresh, and the "New sync"
/// button for the caller to wire up.
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
    let heading = gtk::Label::new(Some("Syncs"));
    heading.add_css_class("title-4");
    heading.set_xalign(0.0);
    let sub = gtk::Label::new(Some("Keep two places in step. Run one and watch it move."));
    sub.add_css_class("dim");
    sub.set_xalign(0.0);
    titles.append(&heading);
    titles.append(&sub);
    let new_btn = gtk::Button::builder()
        .label("New sync")
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

/// Rebuild the job list from the saved jobs.
pub fn refresh(ui: &Rc<Ui>) {
    while let Some(child) = ui.syncs_list.first_child() {
        ui.syncs_list.remove(&child);
    }
    let jobs = esp_core::load_jobs().unwrap_or_default();
    if jobs.is_empty() {
        let row = adw::ActionRow::builder()
            .title("No syncs yet")
            .subtitle("Create a sync to keep two places in step.")
            .build();
        row.add_prefix(&gtk::Image::from_icon_name("emblem-synchronizing-symbolic"));
        ui.syncs_list.append(&row);
        return;
    }
    for job in jobs {
        ui.syncs_list.append(&job_row(ui, job));
    }

    // Headless aids: drive the first job's Run or name-check through the real path once mapped.
    if crate::aid("ESP_AUTORUN").is_some() {
        activate_first("media-playback-start-symbolic", ui);
    }
    if crate::aid("ESP_PREFLIGHT_AUTORUN").is_some() {
        activate_first("edit-find-symbolic", ui);
    }
}

/// Headless aid: activate the first row's button with `icon` shortly after the page maps.
fn activate_first(icon: &'static str, ui: &Rc<Ui>) {
    if let Some(first) = ui.syncs_list.first_child() {
        if let Some(btn) = find_button(&first, icon) {
            glib::timeout_add_local_once(Duration::from_millis(800), move || {
                btn.activate();
            });
        }
    }
}

/// Depth-first search for a button with the given icon within a row (used by headless aids).
pub(crate) fn find_button(root: &gtk::Widget, icon: &str) -> Option<gtk::Button> {
    let mut stack = vec![root.clone()];
    while let Some(w) = stack.pop() {
        if let Ok(btn) = w.clone().downcast::<gtk::Button>() {
            if btn.icon_name().as_deref() == Some(icon) {
                return Some(btn);
            }
        }
        let mut child = w.first_child();
        while let Some(c) = child {
            stack.push(c.clone());
            child = c.next_sibling();
        }
    }
    None
}

/// Depth-first search for the Run button within a job row (used by the headless aid).
pub(crate) fn find_run_button(root: &gtk::Widget) -> Option<gtk::Button> {
    find_button(root, "media-playback-start-symbolic")
}

/// One job row: name, source to destination, a mode badge, a hidden progress bar, and Run.
fn job_row(ui: &Rc<Ui>, job: Job) -> adw::ActionRow {
    let base_subtitle = format!("{} to {}", job.src.display(), job.dst.display());
    let row = adw::ActionRow::builder()
        .title(&job.name)
        .subtitle(&base_subtitle)
        .build();
    row.add_prefix(&gtk::Image::from_icon_name("folder-remote-symbolic"));

    let badge = gtk::Label::new(Some(&job.mode.label().to_uppercase()));
    badge.add_css_class("badge");
    badge.set_valign(gtk::Align::Center);
    if job.mode.is_destructive() {
        badge.add_css_class("danger");
        badge.set_tooltip_text(Some(
            "Mirror deletes files on the destination that are not on the source.",
        ));
    }
    row.add_suffix(&badge);

    // A small badge when the sync runs on a schedule (with the pinned time, if any).
    if job.schedule != esp_core::Schedule::Manual {
        let sched_text = match (job.schedule, job.at) {
            (esp_core::Schedule::Daily, Some(dt)) => format!("Daily {}", dt.label(false)),
            (esp_core::Schedule::Weekly, Some(dt)) => format!("Weekly {}", dt.label(true)),
            (s, _) => s.label().to_string(),
        };
        let sb = gtk::Label::new(Some(&sched_text));
        sb.add_css_class("badge");
        sb.set_valign(gtk::Align::Center);
        sb.set_tooltip_text(Some("Runs automatically while Oortery is open."));
        row.add_suffix(&sb);
    }

    let bar = gtk::ProgressBar::new();
    bar.set_valign(gtk::Align::Center);
    bar.set_width_request(150);
    bar.set_visible(false);
    row.add_suffix(&bar);

    // Pre-flight: catch names a strict cloud will silently drop, before they vanish.
    let check = crate::icon_button("edit-find-symbolic", "Check names for cloud problems");
    check.set_valign(gtk::Align::Center);
    check.add_css_class("flat");
    row.add_suffix(&check);
    let (ui_c, job_c) = (ui.clone(), job.clone());
    check.connect_clicked(move |_| present_preflight(&ui_c, &job_c));

    let run = crate::icon_button("media-playback-start-symbolic", "Run this sync now");
    run.set_valign(gtk::Align::Center);
    run.add_css_class("flat");
    row.add_suffix(&run);

    // Edit / Delete menu.
    let (menu, edit_btn, del_btn, pop) = row_menu();
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
            confirm_delete(&ui_d, &nm, "sync", {
                let (ui2, id2) = (ui_d.clone(), id.clone());
                move || {
                    let mut jobs = esp_core::load_jobs().unwrap_or_default();
                    jobs.retain(|j| j.id != id2);
                    let _ = esp_core::save_jobs(&jobs);
                    refresh(&ui2);
                }
            });
        });
    }

    let ui2 = ui.clone();
    let row2 = row.clone();
    let bar2 = bar.clone();
    let run2 = run.clone();
    run.connect_clicked(move |_| {
        run_job(&ui2, &job, &base_subtitle, &row2, &bar2, &run2);
    });
    row
}

/// Start a job on the daemon and poll its progress into the row until it finishes.
fn run_job(
    ui: &Rc<Ui>,
    job: &Job,
    base_subtitle: &str,
    row: &adw::ActionRow,
    bar: &gtk::ProgressBar,
    run: &gtk::Button,
) {
    let rcd = match ui.ensure_rcd() {
        Ok(r) => r,
        Err(e) => {
            ui.toast(&format!("Could not start the engine: {e}"));
            return;
        }
    };
    let seq = ui.run_seq.get() + 1;
    ui.run_seq.set(seq);
    let group = format!("esp/{}/{seq}", job.id);
    let jobid = match rcd.start_job(job, &group) {
        Ok(id) => id,
        Err(e) => {
            // The cached daemon may have died; drop it so the next run respawns a fresh one.
            *ui.rcd.borrow_mut() = None;
            ui.toast(&format!("Could not start the sync: {e}"));
            return;
        }
    };

    run.set_sensitive(false);
    bar.set_visible(true);
    bar.set_fraction(0.0);
    row.set_subtitle("Starting...");
    ui.active_syncs.set(ui.active_syncs.get() + 1);
    ui.refresh_active_count();

    let ui_p = ui.clone();
    let row_p = row.clone();
    let bar_p = bar.clone();
    let run_p = run.clone();
    let base = base_subtitle.to_string();
    let name = job.name.clone();
    let job_id = job.id.clone();
    glib::timeout_add_local(Duration::from_millis(250), move || {
        let Some(rcd) = ui_p.rcd.borrow().as_ref().cloned() else {
            return glib::ControlFlow::Break;
        };
        let stats = rcd.job_stats(&group);
        // Show real progress once the total is known; until then (scanning, or a backend with no
        // total) pulse so the bar shows motion instead of sitting frozen at 0.
        if stats.total_known() {
            bar_p.set_fraction(stats.fraction());
        } else if stats.active_without_total() {
            bar_p.pulse();
        }
        row_p.set_subtitle(&progress_text(&stats));

        let done = |message: String| {
            run_p.set_sensitive(true);
            bar_p.set_visible(false);
            row_p.set_subtitle(&base);
            let left = ui_p.active_syncs.get().saturating_sub(1);
            ui_p.active_syncs.set(left);
            ui_p.refresh_active_count();
            ui_p.toast(&message);
        };

        match rcd.job_status(jobid) {
            Ok(st) if st.finished => {
                if st.success {
                    let _ = esp_core::runlog::record_success("sync", &job_id);
                    // Tally what this sync moved to safety for the "protected by Oortery" total.
                    let _ = esp_core::record_protected(stats.bytes.max(0) as u64);
                    done(format!("{name} finished"));
                } else {
                    done(format!("{name} failed: {}", st.error));
                }
                glib::ControlFlow::Break
            }
            Ok(_) => glib::ControlFlow::Continue,
            Err(e) => {
                done(format!("{name} lost contact with the engine: {e}"));
                glib::ControlFlow::Break
            }
        }
    });
}

/// Scan the sync's local source for names a strict cloud backend (OneDrive, SharePoint, any
/// case-insensitive target) would silently refuse, and show what it finds. Runs the walk on a
/// worker thread so a big tree never freezes the UI.
fn present_preflight(ui: &Rc<Ui>, job: &Job) {
    if job.src.remote.is_some() {
        ui.toast("Name checks run on a source folder on this computer.");
        return;
    }
    let source = job.src.path.clone();

    let dialog = adw::Dialog::builder()
        .title("Check names")
        .content_width(540)
        .content_height(560)
        .build();
    let list = gtk::ListBox::new();
    list.add_css_class("boxed-list");
    list.set_selection_mode(gtk::SelectionMode::None);
    let spinner = crate::spinner::celestial();
    spinner.set_margin_top(16);
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

    let (tx, rx) = async_channel::bounded(1);
    std::thread::spawn(move || {
        // Cap the report so a pathological tree cannot run away; 200 examples is plenty to act on.
        let out = esp_core::scan_dir(std::path::Path::new(&source), 200).map_err(|e| e.to_string());
        let _ = tx.send_blocking(out);
    });

    let (list_c, spinner_c) = (list.clone(), spinner.clone());
    glib::spawn_future_local(async move {
        let res = rx.recv().await;
        spinner_c.set_visible(false);
        match res {
            Ok(Ok((hazards, _))) if hazards.is_empty() => {
                let row = adw::ActionRow::builder()
                    .title("No problems found")
                    .subtitle("Every name here should sync to any cloud.")
                    .build();
                row.add_prefix(&gtk::Image::from_icon_name("emblem-ok-symbolic"));
                list_c.append(&row);
            }
            Ok(Ok((hazards, capped))) => {
                for h in &hazards {
                    let row = adw::ActionRow::builder()
                        .title(&h.path)
                        .subtitle(h.issue.summary())
                        .build();
                    row.add_prefix(&gtk::Image::from_icon_name("dialog-warning-symbolic"));
                    list_c.append(&row);
                }
                if capped {
                    let row = adw::ActionRow::builder()
                        .title("More not shown")
                        .subtitle("Stopped after the first 200 so the check stays fast.")
                        .build();
                    list_c.append(&row);
                }
            }
            Ok(Err(e)) => {
                let row = adw::ActionRow::builder()
                    .title("Could not check the folder")
                    .subtitle(&e)
                    .build();
                row.add_prefix(&gtk::Image::from_icon_name("dialog-error-symbolic"));
                list_c.append(&row);
            }
            Err(_) => {}
        }
    });

    dialog.present(Some(&ui.window));
}

/// Run a job from the scheduler, with no row to drive. Starts it on the daemon and reports the
/// outcome by toast; the Activity page shows live progress. Keeps the active-sync count honest.
pub(crate) fn run_detached(ui: &Rc<Ui>, job: &Job) {
    let rcd = match ui.ensure_rcd() {
        Ok(r) => r,
        Err(e) => {
            ui.toast(&format!("Scheduled sync could not start the engine: {e}"));
            return;
        }
    };
    let seq = ui.run_seq.get() + 1;
    ui.run_seq.set(seq);
    let group = format!("oortery/{}/{seq}", job.id);
    let jobid = match rcd.start_job(job, &group) {
        Ok(id) => id,
        Err(e) => {
            *ui.rcd.borrow_mut() = None;
            ui.toast(&format!(
                "Scheduled sync '{}' could not start: {e}",
                job.name
            ));
            return;
        }
    };
    ui.active_syncs.set(ui.active_syncs.get() + 1);
    ui.refresh_active_count();
    ui.toast(&format!("Scheduled sync '{}' started", job.name));

    let (ui_p, name, job_id, group_p) = (ui.clone(), job.name.clone(), job.id.clone(), group);
    glib::timeout_add_local(Duration::from_millis(400), move || {
        let Some(rcd) = ui_p.rcd.borrow().as_ref().cloned() else {
            return glib::ControlFlow::Break;
        };
        match rcd.job_status(jobid) {
            Ok(st) if st.finished => {
                let left = ui_p.active_syncs.get().saturating_sub(1);
                ui_p.active_syncs.set(left);
                ui_p.refresh_active_count();
                if st.success {
                    let _ = esp_core::runlog::record_success("sync", &job_id);
                    // Tally what this scheduled sync moved to safety.
                    let moved = rcd.job_stats(&group_p).bytes.max(0) as u64;
                    let _ = esp_core::record_protected(moved);
                    ui_p.toast(&format!("Scheduled '{name}' finished"));
                } else {
                    ui_p.toast(&format!("Scheduled '{name}' failed: {}", st.error));
                }
                glib::ControlFlow::Break
            }
            Ok(_) => glib::ControlFlow::Continue,
            Err(_) => glib::ControlFlow::Break,
        }
    });
}

/// The new-/edit-sync dialog: name, a source and destination built from configured remotes or
/// a local path, and a mode. `edit` prefills it and saves over the same job. Refreshes after.
pub fn present_new(ui: &Rc<Ui>, edit: Option<Job>) {
    let remotes = esp_core::list_remotes().unwrap_or_default();
    // Combo options: index 0 is this computer (local), then each configured remote.
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
            "Edit sync"
        } else {
            "New sync"
        })
        .content_width(560)
        .content_height(620)
        .build();

    let prefs = adw::PreferencesPage::new();

    let gname = adw::PreferencesGroup::builder().title("Sync").build();
    let name_row = adw::EntryRow::builder().title("Name").build();
    gname.add(&name_row);
    prefs.add(&gname);

    let gsrc = adw::PreferencesGroup::builder()
        .title("Source")
        .description("Where files are read from.")
        .build();
    let (src_combo, src_path, src_browse) = endpoint_rows(&label_refs);
    gsrc.add(&src_combo);
    gsrc.add(&src_path);
    prefs.add(&gsrc);
    wire_browse(ui, options.clone(), &src_combo, &src_path, &src_browse);

    let gdst = adw::PreferencesGroup::builder()
        .title("Destination")
        .description("Where files are written to.")
        .build();
    let (dst_combo, dst_path, dst_browse) = endpoint_rows(&label_refs);
    gdst.add(&dst_combo);
    gdst.add(&dst_path);
    prefs.add(&gdst);
    wire_browse(ui, options.clone(), &dst_combo, &dst_path, &dst_browse);

    let gmode = adw::PreferencesGroup::builder().title("Mode").build();
    let mode_model = gtk::StringList::new(&[
        "Copy (add and update, never delete)",
        "Mirror (make identical, deletes extras)",
    ]);
    let mode_combo = adw::ComboRow::builder()
        .title("How to reconcile")
        .model(&mode_model)
        .build();
    gmode.add(&mode_combo);
    let warn = adw::ActionRow::builder()
        .title("Mirror deletes files")
        .subtitle(
            "Anything on the destination that is not on the source is removed. Use with care.",
        )
        .build();
    warn.add_prefix(&gtk::Image::from_icon_name("dialog-warning-symbolic"));
    warn.set_visible(false);
    gmode.add(&warn);
    let warn_c = warn.clone();
    mode_combo.connect_selected_notify(move |c| {
        warn_c.set_visible(c.selected() == 1);
    });
    prefs.add(&gmode);

    // When: run on its own, on a schedule (while Oortery is open), optionally at a chosen time.
    let (init_sched, init_at) = edit
        .as_ref()
        .map(|j| (j.schedule, j.at))
        .unwrap_or((esp_core::Schedule::Manual, None));
    let (gwhen, sched_read) = schedule_picker(init_sched, init_at);
    prefs.add(&gwhen);

    // Prefill when editing an existing job.
    if let Some(j) = &edit {
        name_row.set_text(&j.name);
        src_combo.set_selected(combo_index_for(&options, &j.src));
        src_path.set_text(&j.src.path);
        dst_combo.set_selected(combo_index_for(&options, &j.dst));
        dst_path.set_text(&j.dst.path);
        mode_combo.set_selected(u32::from(j.mode == SyncMode::Mirror));
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
    // Preserve the last-run time across an edit so changing the schedule does not re-trigger it.
    let prev_last_run = edit.as_ref().and_then(|j| j.last_run);
    create.connect_clicked(move |_| {
        let name = name_row.text().trim().to_string();
        if name.is_empty() {
            ui2.toast("Give the sync a name.");
            return;
        }
        let src = endpoint_from(&options, src_combo.selected(), &src_path.text());
        let dst = endpoint_from(&options, dst_combo.selected(), &dst_path.text());
        if let Err(msg) = validate(&src, "Source").and_then(|_| validate(&dst, "Destination")) {
            ui2.toast(&msg);
            return;
        }
        let mode = if mode_combo.selected() == 1 {
            SyncMode::Mirror
        } else {
            SyncMode::Copy
        };
        let (schedule, at) = sched_read();
        let mut jobs = esp_core::load_jobs().unwrap_or_default();
        let id = match &edit_id {
            Some(id) => id.clone(),
            None => esp_core::slug_id(&name, &jobs),
        };
        let new_job = Job {
            id: id.clone(),
            name: name.clone(),
            src,
            dst,
            mode,
            schedule,
            last_run: prev_last_run,
            at,
        };
        match jobs.iter().position(|j| j.id == id) {
            Some(pos) => jobs[pos] = new_job,
            None => jobs.push(new_job),
        }
        match esp_core::save_jobs(&jobs) {
            Ok(()) => {
                dlg2.close();
                refresh(&ui2);
                ui2.toast(&format!("Saved {name}"));
                crate::prefs::nudge_run_on_time(&ui2, schedule);
            }
            Err(e) => ui2.toast(&format!("Could not save: {e}")),
        }
    });

    dialog.present(Some(&ui.window));
}

/// A remote-or-local combo, a path entry, and a Browse button (returned to be wired).
pub(crate) fn endpoint_rows(labels: &[&str]) -> (adw::ComboRow, adw::EntryRow, gtk::Button) {
    let model = gtk::StringList::new(labels);
    let combo = adw::ComboRow::builder()
        .title("Location")
        .model(&model)
        .build();
    let path = adw::EntryRow::builder().title("Path").build();
    let browse = crate::icon_button("folder-open-symbolic", "Browse folders");
    browse.set_valign(gtk::Align::Center);
    browse.add_css_class("flat");
    path.add_suffix(&browse);
    (combo, path, browse)
}

/// Open the folder browser for this side's selected location, writing the pick back to `path`.
pub(crate) fn wire_browse(
    ui: &Rc<Ui>,
    options: Vec<Option<String>>,
    combo: &adw::ComboRow,
    path: &adw::EntryRow,
    browse: &gtk::Button,
) {
    let ui = ui.clone();
    let combo = combo.clone();
    let path = path.clone();
    browse.connect_clicked(move |_| {
        let remote = options
            .get(combo.selected() as usize)
            .and_then(|o| o.clone());
        let is_local = remote.is_none();
        let fs = match &remote {
            Some(r) => format!("{r}:"),
            None => "/".to_string(),
        };
        // Start where the entry already points (local paths are absolute; the fs root is "/").
        // For a fresh local pick, open at the user's home (OS-aware) since that is what most
        // people back up; the browser can still navigate up to the root.
        let cur = path.text().to_string();
        let start = if is_local {
            if cur.is_empty() {
                crate::folder_browser::local_default_start()
            } else {
                cur.trim_start_matches('/').to_string()
            }
        } else if cur.is_empty() {
            // Land in the remote's bucket (its stored root) instead of the account root, which a
            // bucket-scoped key cannot list.
            remote
                .as_deref()
                .and_then(esp_core::roots::get)
                .unwrap_or_default()
        } else {
            cur
        };
        let path_w = path.clone();
        let on_pick: Rc<dyn Fn(String)> = Rc::new(move |sub: String| {
            path_w.set_text(&if is_local { format!("/{sub}") } else { sub });
        });
        crate::folder_browser::present(&ui, fs, start, on_pick);
    });
}

/// The combo index for a schedule (position in `Schedule::ALL`). Shared by both dialogs.
pub(crate) fn sched_index(s: esp_core::Schedule) -> u32 {
    esp_core::Schedule::ALL
        .iter()
        .position(|&x| x == s)
        .unwrap_or(0) as u32
}

/// Reads the current schedule choice (cadence and optional pinned time) from a schedule picker.
pub(crate) type ScheduleRead = Rc<dyn Fn() -> (esp_core::Schedule, Option<esp_core::DayTime>)>;

/// A schedule picker for the New-sync / New-backup dialogs: the cadence chooser, plus a time of day
/// (and a weekday for Weekly) that appear only when Daily or Weekly is chosen. Returns the group to
/// drop into a dialog and a reader for the current choice. Shared by both dialogs.
pub(crate) fn schedule_picker(
    initial: esp_core::Schedule,
    at: Option<esp_core::DayTime>,
) -> (adw::PreferencesGroup, ScheduleRead) {
    use esp_core::{DayTime, Schedule};
    let group = adw::PreferencesGroup::builder().title("Schedule").build();
    let labels: Vec<&str> = Schedule::ALL.iter().map(|s| s.label()).collect();
    let combo = adw::ComboRow::builder()
        .title("Run automatically")
        .subtitle("Runs while Oortery is open.")
        .model(&gtk::StringList::new(&labels))
        .build();
    combo.set_selected(sched_index(initial));
    group.add(&combo);

    const DAYS: [&str; 7] = [
        "Sunday",
        "Monday",
        "Tuesday",
        "Wednesday",
        "Thursday",
        "Friday",
        "Saturday",
    ];
    let weekday = adw::ComboRow::builder()
        .title("Day of week")
        .model(&gtk::StringList::new(&DAYS))
        .build();
    group.add(&weekday);

    let hour = adw::SpinRow::builder()
        .title("Hour")
        .adjustment(&gtk::Adjustment::new(2.0, 0.0, 23.0, 1.0, 5.0, 0.0))
        .build();
    let minute = adw::SpinRow::builder()
        .title("Minute")
        .adjustment(&gtk::Adjustment::new(0.0, 0.0, 59.0, 1.0, 10.0, 0.0))
        .build();
    group.add(&hour);
    group.add(&minute);

    if let Some(dt) = at {
        hour.set_value(dt.hour as f64);
        minute.set_value(dt.minute as f64);
        weekday.set_selected(dt.weekday as u32);
    }

    let vis = {
        let (w, h, m) = (weekday.clone(), hour.clone(), minute.clone());
        move |s: Schedule| {
            let timed = matches!(s, Schedule::Daily | Schedule::Weekly);
            h.set_visible(timed);
            m.set_visible(timed);
            w.set_visible(matches!(s, Schedule::Weekly));
        }
    };
    vis(initial);
    combo.connect_selected_notify(move |c| {
        let s = *Schedule::ALL
            .get(c.selected() as usize)
            .unwrap_or(&Schedule::Manual);
        vis(s);
    });

    let read: ScheduleRead = {
        let (combo, weekday, hour, minute) =
            (combo.clone(), weekday.clone(), hour.clone(), minute.clone());
        Rc::new(move || {
            let s = *Schedule::ALL
                .get(combo.selected() as usize)
                .unwrap_or(&Schedule::Manual);
            let at = match s {
                Schedule::Daily => Some(DayTime {
                    hour: hour.value() as u8,
                    minute: minute.value() as u8,
                    weekday: 0,
                }),
                Schedule::Weekly => Some(DayTime {
                    hour: hour.value() as u8,
                    minute: minute.value() as u8,
                    weekday: weekday.selected() as u8,
                }),
                _ => None,
            };
            (s, at)
        })
    };
    (group, read)
}

/// Build an endpoint from the selected combo option and a path string.
pub(crate) fn endpoint_from(options: &[Option<String>], selected: u32, path: &str) -> Endpoint {
    let path = path.trim().to_string();
    match options.get(selected as usize).and_then(|o| o.clone()) {
        Some(remote) => Endpoint::remote(remote, path),
        None => Endpoint::local(path),
    }
}

/// The combo index matching an endpoint's remote (0 = this computer).
pub(crate) fn combo_index_for(options: &[Option<String>], ep: &Endpoint) -> u32 {
    options.iter().position(|o| o == &ep.remote).unwrap_or(0) as u32
}

/// A "more actions" menu button with Edit and Delete items. Returns the button plus the two
/// action buttons and the popover for the caller to wire. Shared by syncs and backups.
pub(crate) fn row_menu() -> (gtk::MenuButton, gtk::Button, gtk::Button, gtk::Popover) {
    let menu = gtk::MenuButton::builder()
        .icon_name("view-more-symbolic")
        .valign(gtk::Align::Center)
        .tooltip_text("More actions (edit, delete)")
        .build();
    menu.update_property(&[gtk::accessible::Property::Label("More actions")]);
    menu.add_css_class("flat");
    let b = gtk::Box::new(gtk::Orientation::Vertical, 2);
    b.set_margin_top(4);
    b.set_margin_bottom(4);
    b.set_margin_start(4);
    b.set_margin_end(4);
    let edit = gtk::Button::with_label("Edit");
    edit.add_css_class("flat");
    edit.set_halign(gtk::Align::Fill);
    let del = gtk::Button::with_label("Delete");
    del.add_css_class("flat");
    del.set_halign(gtk::Align::Fill);
    b.append(&edit);
    b.append(&del);
    let pop = gtk::Popover::new();
    pop.set_child(Some(&b));
    menu.set_popover(Some(&pop));
    (menu, edit, del, pop)
}

/// Confirm a deletion, running `on_confirm` only if the user agrees. `noun` is "sync"/"backup".
pub(crate) fn confirm_delete(ui: &Rc<Ui>, name: &str, noun: &str, on_confirm: impl Fn() + 'static) {
    let dialog = adw::AlertDialog::new(
        Some(&format!("Delete {name}?")),
        Some(&format!(
            "This removes the {noun}. It does not delete any of your files."
        )),
    );
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("delete", "Delete");
    dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");
    let (ui2, name2) = (ui.clone(), name.to_string());
    dialog.connect_response(None, move |_, resp| {
        if resp == "delete" {
            on_confirm();
            ui2.toast(&format!("Deleted {name2}"));
        }
    });
    dialog.present(Some(&ui.window));
}

/// A local endpoint needs a path; a remote endpoint may point at the whole remote.
fn validate(ep: &Endpoint, side: &str) -> Result<(), String> {
    if ep.remote.is_none() && ep.path.is_empty() {
        return Err(format!("{side}: choose a path on this computer."));
    }
    Ok(())
}

fn progress_text(s: &JobStats) -> String {
    let moved = human_bytes(s.bytes);
    let total = if s.total_bytes > 0 {
        human_bytes(s.total_bytes)
    } else {
        "?".to_string()
    };
    let rate = format!("{}/s", human_bytes(s.speed.max(0.0) as i64));
    match s.eta {
        Some(eta) if eta > 0.0 => format!(
            "{moved} of {total} \u{00b7} {rate} \u{00b7} ETA {}",
            human_secs(eta)
        ),
        _ => format!("{moved} of {total} \u{00b7} {rate}"),
    }
}

pub(crate) fn human_bytes(n: i64) -> String {
    const UNITS: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];
    if n < 1024 {
        return format!("{n} B");
    }
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    format!("{v:.1} {}", UNITS[i])
}

pub(crate) fn human_secs(secs: f64) -> String {
    let s = secs as i64;
    if s < 60 {
        format!("{s}s")
    } else if s < 3600 {
        format!("{}m {}s", s / 60, s % 60)
    } else {
        format!("{}h {}m", s / 3600, (s % 3600) / 60)
    }
}
