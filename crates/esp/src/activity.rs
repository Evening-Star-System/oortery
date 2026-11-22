//! The Activity page: a live look at what the engine is moving right now. It polls the shared
//! rcd daemon's `core/stats` and shows each in-flight transfer with its own progress bar,
//! updating rows in place so it flows instead of flickering.

use crate::{syncs, Ui};
use adw::prelude::*;
use esp_core::StallTracker;
use gtk::glib;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::{Duration, Instant};

/// Build the Activity page. Returns the page, the total-throughput label, and the list.
pub fn page() -> (gtk::Widget, gtk::Label, gtk::ListBox, gtk::ToggleButton) {
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
    let heading = gtk::Label::new(Some("Activity"));
    heading.add_css_class("title-4");
    heading.set_xalign(0.0);
    let sub = gtk::Label::new(Some("What your data is doing right now."));
    sub.add_css_class("dim");
    sub.set_xalign(0.0);
    titles.append(&heading);
    titles.append(&sub);

    let speed = gtk::Label::new(None);
    speed.add_css_class("title-2");
    speed.add_css_class("stat-accent");
    speed.set_valign(gtk::Align::Center);

    // Pause/resume all transfers (throttles the engine to a crawl, then lifts it). Wired in main.
    let pause = gtk::ToggleButton::with_label("Pause");
    pause.set_valign(gtk::Align::Center);
    pause.set_tooltip_text(Some("Pause all transfers, then resume where they left off"));

    header.append(&titles);
    header.append(&speed);
    header.append(&pause);
    header.set_margin_bottom(6);

    let list = gtk::ListBox::new();
    list.add_css_class("boxed-list");
    list.set_selection_mode(gtk::SelectionMode::None);

    col.append(&header);
    col.append(&list);
    scroll.set_child(Some(&col));
    clamp.set_child(Some(&scroll));
    (clamp.upcast(), speed, list, pause)
}

/// Start the live poll. Runs for the app's lifetime; only touches the daemon when it exists.
pub fn start_polling(ui: &Rc<Ui>) {
    let rows: Rc<RefCell<HashMap<String, (adw::ActionRow, gtk::ProgressBar)>>> =
        Rc::new(RefCell::new(HashMap::new()));
    let placeholder: Rc<RefCell<Option<adw::ActionRow>>> = Rc::new(RefCell::new(None));
    // Detect transfers frozen at 0 B/s so a silent hang is visible, not mistaken for slow.
    let stalls = Rc::new(RefCell::new(StallTracker::new(
        esp_core::DEFAULT_STALL_SECS,
    )));
    let started = Instant::now();
    let ui = ui.clone();

    glib::timeout_add_local(Duration::from_millis(600), move || {
        // Read the port, drop the borrow, then query (a short subprocess) without holding it.
        let port = ui.rcd.borrow().as_ref().map(|r| r.port());
        let act = port.map(esp_core::activity).unwrap_or_default();
        let stalled = stalls
            .borrow_mut()
            .observe(started.elapsed().as_secs_f64(), &act.transfers);

        let speed_text = if act.speed > 1.0 {
            format!("{}/s", syncs::human_bytes(act.speed as i64))
        } else {
            String::new()
        };
        ui.activity_speed.set_text(&speed_text);

        let mut map = rows.borrow_mut();
        let mut seen = HashSet::new();
        for t in &act.transfers {
            seen.insert(t.name.clone());
            let frac = (t.percentage / 100.0).clamp(0.0, 1.0);
            let is_stalled = stalled.contains(&t.name);
            let subtitle = if is_stalled {
                format!("Stalled \u{00b7} no progress \u{00b7} {}", transfer_sub(t))
            } else {
                transfer_sub(t)
            };
            if let Some((row, bar)) = map.get(&t.name) {
                bar.set_fraction(frac);
                row.set_subtitle(&subtitle);
                mark_stalled(row, is_stalled);
            } else {
                let row = adw::ActionRow::builder()
                    .title(&t.name)
                    .subtitle(&subtitle)
                    .build();
                row.add_prefix(&gtk::Image::from_icon_name("folder-download-symbolic"));
                let bar = gtk::ProgressBar::new();
                bar.set_valign(gtk::Align::Center);
                bar.set_width_request(160);
                bar.set_fraction(frac);
                row.add_suffix(&bar);
                ui.activity_list.append(&row);
                mark_stalled(&row, is_stalled);
                map.insert(t.name.clone(), (row, bar));
            }
        }
        // Drop rows for transfers that finished.
        let gone: Vec<String> = map.keys().filter(|k| !seen.contains(*k)).cloned().collect();
        for k in gone {
            if let Some((row, _)) = map.remove(&k) {
                ui.activity_list.remove(&row);
            }
        }

        // Idle placeholder when nothing is moving.
        let empty = map.is_empty();
        drop(map);
        let mut ph = placeholder.borrow_mut();
        if empty && ph.is_none() {
            let row = adw::ActionRow::builder()
                .title("Nothing moving right now")
                .subtitle("Run a sync and watch it flow here.")
                .build();
            row.add_prefix(&gtk::Image::from_icon_name("weather-clear-night-symbolic"));
            ui.activity_list.append(&row);
            *ph = Some(row);
        } else if !empty {
            if let Some(row) = ph.take() {
                ui.activity_list.remove(&row);
            }
        }

        glib::ControlFlow::Continue
    });
}

/// Toggle the warning styling on a row whose transfer is stalled, so a frozen transfer reads
/// as a problem at a glance rather than just slow.
fn mark_stalled(row: &adw::ActionRow, stalled: bool) {
    if stalled {
        row.add_css_class("warning");
    } else {
        row.remove_css_class("warning");
    }
}

fn transfer_sub(t: &esp_core::Transfer) -> String {
    let moved = syncs::human_bytes(t.bytes);
    let total = syncs::human_bytes(t.size);
    let rate = format!("{}/s", syncs::human_bytes(t.speed.max(0.0) as i64));
    match t.eta {
        Some(e) if e > 0.0 => format!(
            "{moved} of {total} \u{00b7} {rate} \u{00b7} ETA {}",
            syncs::human_secs(e)
        ),
        _ => format!("{moved} of {total} \u{00b7} {rate}"),
    }
}
