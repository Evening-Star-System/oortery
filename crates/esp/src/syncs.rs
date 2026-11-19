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

    // Headless aid: drive the first job's Run through the real path once mapped.
    if std::env::var("ESP_AUTORUN").is_ok() {
        if let Some(first) = ui.syncs_list.first_child() {
            if let Some(btn) = find_run_button(&first) {
                glib::timeout_add_local_once(Duration::from_millis(800), move || {
                    btn.activate();
                });
            }
        }
    }
}

/// Depth-first search for the Run button within a job row (used by the headless aid).
pub(crate) fn find_run_button(root: &gtk::Widget) -> Option<gtk::Button> {
    let mut stack = vec![root.clone()];
    while let Some(w) = stack.pop() {
        if let Ok(btn) = w.clone().downcast::<gtk::Button>() {
            if btn.icon_name().as_deref() == Some("media-playback-start-symbolic") {
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

    let bar = gtk::ProgressBar::new();
    bar.set_valign(gtk::Align::Center);
    bar.set_width_request(150);
    bar.set_visible(false);
    row.add_suffix(&bar);

    let run = gtk::Button::from_icon_name("media-playback-start-symbolic");
    run.set_tooltip_text(Some("Run this sync now"));
    run.set_valign(gtk::Align::Center);
    run.add_css_class("flat");
    row.add_suffix(&run);

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
    glib::timeout_add_local(Duration::from_millis(250), move || {
        let Some(rcd) = ui_p.rcd.borrow().as_ref().cloned() else {
            return glib::ControlFlow::Break;
        };
        let stats = rcd.job_stats(&group);
        bar_p.set_fraction(stats.fraction());
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

/// The new-sync dialog: name, a source and destination built from configured remotes or a
/// local path, and a mode. Validates and saves, then refreshes the list.
pub fn present_new(ui: &Rc<Ui>) {
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
        .title("New sync")
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

    let scroll = gtk::ScrolledWindow::new();
    scroll.set_vexpand(true);
    scroll.set_child(Some(&prefs));

    let header = adw::HeaderBar::new();
    let create = gtk::Button::with_label("Create");
    create.add_css_class("suggested-action");
    header.pack_end(&create);
    let tv = adw::ToolbarView::new();
    tv.add_top_bar(&header);
    tv.set_content(Some(&scroll));
    dialog.set_child(Some(&tv));

    let ui2 = ui.clone();
    let dlg2 = dialog.clone();
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
        let mut jobs = esp_core::load_jobs().unwrap_or_default();
        let id = esp_core::slug_id(&name, &jobs);
        jobs.push(Job {
            id,
            name: name.clone(),
            src,
            dst,
            mode,
        });
        match esp_core::save_jobs(&jobs) {
            Ok(()) => {
                dlg2.close();
                refresh(&ui2);
                ui2.toast(&format!("Saved {name}"));
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
    let browse = gtk::Button::from_icon_name("folder-open-symbolic");
    browse.set_tooltip_text(Some("Browse folders"));
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
        let cur = path.text().to_string();
        let start = if is_local {
            cur.trim_start_matches('/').to_string()
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

/// Build an endpoint from the selected combo option and a path string.
pub(crate) fn endpoint_from(options: &[Option<String>], selected: u32, path: &str) -> Endpoint {
    let path = path.trim().to_string();
    match options.get(selected as usize).and_then(|o| o.clone()) {
        Some(remote) => Endpoint::remote(remote, path),
        None => Endpoint::local(path),
    }
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
