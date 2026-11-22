//! Oortery: a mission control for all your data.
//!
//! A native GTK4 + libadwaita shell over esp-core. Overview with a live data
//! constellation, a Remotes page from the real rclone config, and an add-remote flow
//! that spans every backend rclone can reach.

mod about;
mod activity;
mod add_remote;
mod backups;
mod constellation;
mod diagnostics;
mod folder_browser;
mod lockscreen;
mod mounts;
mod prefs;
mod spinner;
mod syncs;

use adw::prelude::*;
use gtk::{gdk, glib};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

const APP_ID: &str = "app.eveningstar.Oortery";

/// Shared, refreshable UI handles passed to dialogs and refresh logic.
pub struct Ui {
    pub window: adw::ApplicationWindow,
    pub toasts: adw::ToastOverlay,
    pub providers: Vec<esp_core::Provider>,
    pub remotes_list: gtk::ListBox,
    pub remotes_count: gtk::Label,
    pub node_count: Rc<Cell<usize>>,
    pub syncs_list: gtk::ListBox,
    pub mounts_list: gtk::ListBox,
    pub backups_list: gtk::ListBox,
    pub activity_list: gtk::ListBox,
    pub activity_speed: gtk::Label,
    pub active_label: gtk::Label,
    // Overview health banner + freshness/coverage tiles (status-first, per the dashboard research).
    pub health_banner: gtk::Box,
    pub health_icon: gtk::Image,
    pub health_text: gtk::Label,
    pub health_sub: gtk::Label,
    pub last_backup_label: gtk::Label,
    pub coverage_label: gtk::Label,
    pub overview_onboarding: gtk::Box,
    pub overview_tiles: gtk::Box,
    /// The operations daemon, started lazily the first time a job runs.
    pub rcd: RefCell<Option<Rc<esp_core::Rcd>>>,
    /// Number of jobs currently running (drives the Overview stat).
    pub active_syncs: Rc<Cell<usize>>,
    /// Monotonic counter so each run gets a fresh stats group.
    pub run_seq: Cell<u64>,
}

impl Ui {
    pub fn toast(&self, msg: &str) {
        esp_core::diag::info(msg);
        self.toasts.add_toast(adw::Toast::new(msg));
    }

    /// A failure message: recorded at Error level in the diagnostics log (which the user can copy or
    /// save to send us) and shown with a longer, more noticeable toast so it is not missed.
    pub fn error(&self, msg: &str) {
        esp_core::diag::error(msg);
        let t = adw::Toast::new(msg);
        t.set_timeout(8);
        self.toasts.add_toast(t);
    }

    /// The shared operations daemon, starting it on first use.
    pub fn ensure_rcd(&self) -> anyhow::Result<Rc<esp_core::Rcd>> {
        if let Some(rcd) = self.rcd.borrow().as_ref() {
            return Ok(rcd.clone());
        }
        let rcd = Rc::new(esp_core::Rcd::start()?);
        *self.rcd.borrow_mut() = Some(rcd.clone());
        Ok(rcd)
    }

    /// Reflect the running-job count in the Overview stat.
    pub fn refresh_active_count(&self) {
        self.active_label
            .set_text(&self.active_syncs.get().to_string());
    }
}

/// Re-read the rclone config and update the remotes list, the stat count, and the
/// constellation node count (the hero redraws itself every frame).
pub fn refresh(ui: &Rc<Ui>) {
    let remotes = esp_core::list_remotes().unwrap_or_default();
    while let Some(child) = ui.remotes_list.first_child() {
        ui.remotes_list.remove(&child);
    }
    if remotes.is_empty() {
        let row = adw::ActionRow::builder()
            .title("No remotes yet")
            .subtitle("Click + to connect your first backend.")
            .build();
        row.add_prefix(&gtk::Image::from_icon_name("network-server-symbolic"));
        ui.remotes_list.append(&row);
    } else {
        for r in &remotes {
            let row = adw::ActionRow::builder()
                .title(&r.name)
                .subtitle(esp_core::backend_label(&r.backend))
                .build();
            row.add_prefix(&gtk::Image::from_icon_name("folder-remote-symbolic"));
            let tag = gtk::Label::new(Some(&r.backend));
            tag.add_css_class("dim");
            tag.add_css_class("caption");
            row.add_suffix(&tag);
            // Edit this remote's settings (prefilled form, saved with rclone config update).
            let edit = gtk::Button::from_icon_name("document-edit-symbolic");
            edit.set_tooltip_text(Some("Edit this remote"));
            edit.set_valign(gtk::Align::Center);
            edit.add_css_class("flat");
            let (ui_e, name_e) = (ui.clone(), r.name.clone());
            edit.connect_clicked(move |_| add_remote::present_edit(&ui_e, &name_e));
            row.add_suffix(&edit);
            // Remove this remote (its connection config only; never the files in the cloud).
            let remove = gtk::Button::from_icon_name("user-trash-symbolic");
            remove.set_tooltip_text(Some("Remove this remote"));
            remove.set_valign(gtk::Align::Center);
            remove.add_css_class("flat");
            let (ui_r, name_r) = (ui.clone(), r.name.clone());
            remove.connect_clicked(move |_| {
                mounts::confirm_remove_remote(&ui_r, &name_r);
            });
            row.add_suffix(&remove);
            ui.remotes_list.append(&row);
        }
    }
    ui.remotes_count.set_text(&remotes.len().to_string());
    ui.node_count.set(remotes.len().max(6));
    update_overview(ui);
}

/// Refresh every page that reflects remotes, so adding or removing one updates the whole app
/// (Remotes, Mounts, Syncs, Backups, Overview) in one call, not just the page you are on.
pub fn refresh_all(ui: &Rc<Ui>) {
    refresh(ui);
    mounts::refresh(ui);
    syncs::refresh(ui);
    backups::refresh(ui);
}

/// Recompute the Overview health verdict and freshness/coverage tiles from the (cheap, local)
/// protection summary. Called whenever remotes, syncs, or backups change. Status-first, per the
/// dashboard research: one "are we OK?" signal, named issues, semantic colour plus an icon.
pub fn update_overview(ui: &Rc<Ui>) {
    let s = esp_core::summary();

    for class in ["healthy", "attention", "firstrun"] {
        ui.health_banner.remove_css_class(class);
    }
    let (class, icon) = match s.health {
        esp_core::Health::Healthy => ("healthy", "emblem-ok-symbolic"),
        esp_core::Health::Attention => ("attention", "dialog-warning-symbolic"),
        esp_core::Health::FirstRun => ("firstrun", "weather-clear-night-symbolic"),
    };
    ui.health_banner.add_css_class(class);
    ui.health_icon.set_icon_name(Some(icon));
    ui.health_text.set_text(&s.headline);

    // First run shows the welcome launcher; once set up, the actionable tiles.
    let first = s.health == esp_core::Health::FirstRun;
    ui.overview_onboarding.set_visible(first);
    ui.overview_tiles.set_visible(!first);

    // Secondary line: freshness when we have it, else gentle guidance.
    let sub = match s.newest_backup_age {
        Some(age) => format!("Last backup {}", esp_core::ago(age)),
        None if s.backups > 0 => "No backups have run yet".to_string(),
        None => "Your data is not backed up yet".to_string(),
    };
    ui.health_sub.set_text(&sub);

    // Tiles.
    ui.last_backup_label.set_text(
        &s.newest_backup_age
            .map(esp_core::ago)
            .unwrap_or_else(|| "never".to_string()),
    );
    ui.coverage_label
        .set_text(&format!("{}/{}", s.backups_run, s.backups));
}

/// Prepend this executable's own directory to PATH, so a bundled rclone/restic (shipped beside
/// the binary in a Flatpak/AppImage) is found even when the launcher leaves it off PATH.
/// A sensible default folder for saving or exporting a file: the user's Downloads if it exists,
/// otherwise their home. OS-aware via `esp_core::home_dir`.
pub fn default_save_dir() -> Option<std::path::PathBuf> {
    let home = esp_core::home_dir()?;
    let downloads = home.join("Downloads");
    Some(if downloads.is_dir() { downloads } else { home })
}

fn prefer_bundled_tools() {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let Some(dir) = exe.parent() else {
        return;
    };
    let mut paths = vec![dir.to_path_buf()];
    if let Some(existing) = std::env::var_os("PATH") {
        paths.extend(std::env::split_paths(&existing));
    }
    if let Ok(joined) = std::env::join_paths(paths) {
        std::env::set_var("PATH", joined);
    }
}

/// Read a headless test aid (an `ESP_*` env var), but ONLY in a debug build. In a release build
/// (the shipped AppImage) this always returns `None`, so the aids cannot be driven from the
/// ambient environment: `ESP_NO_LOCK` can never bypass the app lock and no `*_AUTORUN` can fire.
/// Dev and the Xvfb verification run debug builds, where the aids keep working.
#[cfg(debug_assertions)]
pub(crate) fn aid(name: &str) -> Option<String> {
    std::env::var(name).ok()
}
#[cfg(not(debug_assertions))]
pub(crate) fn aid(_name: &str) -> Option<String> {
    None
}

fn main() -> glib::ExitCode {
    // Find our bundled rclone and restic. In a Flatpak/AppImage they sit next to this binary,
    // but the launcher does not put that dir on PATH, so on a machine with no system rclone the
    // app would see zero providers and no backups. Prepend our own directory so the bundled
    // tools win (and are found at all). Harmless in dev, where system tools stay reachable.
    prefer_bundled_tools();

    // Oortery always wears its night sky. Clear any inherited GTK_THEME (an AppImage launcher,
    // or a user's shell) before GTK starts, so nothing overrides our forced dark scheme.
    std::env::remove_var("GTK_THEME");

    // Start a fresh diagnostics log for this session (local only, never transmitted).
    esp_core::diag::start_session();
    esp_core::diag::info("Oortery started");

    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_startup(|_| {
        // Oortery lives in a night sky: force the dark scheme so the look is consistent.
        adw::StyleManager::default().set_color_scheme(adw::ColorScheme::ForceDark);
        // Use our own icon (the static evening-star glyph) for the window, taskbar, and Activities,
        // so it is not left to app-id/theme matching that can miss on some setups.
        gtk::Window::set_default_icon_name(APP_ID);
        load_css();
    });
    app.connect_activate(build_ui);
    app.run()
}

fn load_css() {
    let provider = gtk::CssProvider::new();
    provider.load_from_string(
        "
        /* Evening Star palette: periwinkle for text accents, violet for actions. */
        @define-color accent_color #b4baff;
        @define-color accent_bg_color #6d5cf0;
        @define-color accent_fg_color #ffffff;

        /* The whole app sits in a night sky: a deep indigo-violet base with a soft glow
           bleeding down from the top, so every page feels celestial, not just the hero. */
        window {
          background-color: #0d0b1a;
          background-image: radial-gradient(900px 520px at 50% -8%, alpha(#312a66, 0.70), transparent 72%),
                            radial-gradient(1100px 560px at 50% 112%, alpha(#2a1f4d, 0.45), transparent 70%);
        }
        .navigation-sidebar { background: alpha(#0a0916, 0.55); }
        headerbar { background: alpha(#0c0a18, 0.72); box-shadow: none; }

        .hero { border-radius: 18px; }
        .dim { opacity: 0.6; }

        /* The 'are we OK?' banner. Status is carried by an icon AND a tinted left edge + glow,
           never colour alone. Periwinkle is the brand accent, so red/amber stay free for alerts. */
        .health-banner { padding: 16px 20px; border-radius: 16px;
                         border: 1px solid alpha(@accent_color, 0.12);
                         border-left: 4px solid alpha(@accent_color, 0.5);
                         background: linear-gradient(to right, alpha(#8a7cf0, 0.10), alpha(#ffffff, 0.02)); }
        .health-banner.healthy  { border-left-color: #4fd196;
                         background: linear-gradient(to right, alpha(#2fbf7f, 0.16), alpha(#ffffff, 0.02)); }
        .health-banner.healthy image { color: #6fe3ad; }
        .health-banner.attention { border-left-color: #f0b94f;
                         background: linear-gradient(to right, alpha(#f0a84f, 0.18), alpha(#ffffff, 0.02)); }
        .health-banner.attention image { color: #ffcf7a; }
        .health-banner.firstrun image { color: @accent_color; }

        /* Stat cards: a faint top-lit glass panel with a periwinkle edge. */
        .stat-card { padding: 18px 22px; min-width: 150px; border-radius: 16px;
                     background: linear-gradient(to bottom, alpha(#ffffff, 0.06), alpha(#8a7cf0, 0.03));
                     border: 1px solid alpha(@accent_color, 0.12); }
        .stat-number { font-size: 2.3rem; font-weight: 800; }
        .stat-caption { opacity: 0.55; font-size: 0.82rem; letter-spacing: 0.04em; }
        .stat-accent { color: @accent_color; }

        /* Selected nav reads as a soft periwinkle pill. */
        row.nav { border-radius: 10px; margin: 1px 6px; }
        row.nav:selected { background: alpha(@accent_color, 0.16); color: #ffffff; }

        /* Primary actions glow like the evening star. */
        button.suggested-action {
          background: linear-gradient(to bottom, #7d6ef7, #5d4fe0);
          color: #ffffff; border: none;
          box-shadow: 0 1px 10px alpha(#6d5cf0, 0.45);
        }
        button.suggested-action:hover {
          background: linear-gradient(to bottom, #8a7cff, #6a5ce8);
          box-shadow: 0 1px 14px alpha(#7d6ef7, 0.60);
        }

        .badge { font-size: 0.72rem; font-weight: 700; letter-spacing: 0.03em;
                 padding: 2px 9px; border-radius: 9px; background: alpha(@accent_color, 0.16);
                 color: @accent_color; }
        .badge.danger { background: alpha(@destructive_color, 0.18); color: @destructive_color; }

        /* Boxed lists as faint glass so content floats on the sky. */
        .boxed-list { background: alpha(#ffffff, 0.03); border: 1px solid alpha(@accent_color, 0.08); }
        ",
    );
    if let Some(display) = gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}

/// Run scheduled syncs and backups that have come due, while the app is open. Ticks once a
/// minute; reloads from disk each tick (picking up edits), and writes `last_run` back on a
/// fresh load so it never clobbers a job the user just added or edited.
fn start_scheduler(ui: &Rc<Ui>) {
    let ui = ui.clone();
    glib::timeout_add_local(std::time::Duration::from_secs(60), move || {
        let now = esp_core::now_unix();

        let jobs = esp_core::load_jobs().unwrap_or_default();
        let due: Vec<_> = jobs
            .iter()
            .filter(|j| j.schedule.is_due_at(j.at, j.last_run, now))
            .cloned()
            .collect();
        if !due.is_empty() {
            for j in &due {
                syncs::run_detached(&ui, j);
            }
            let ids: Vec<&str> = due.iter().map(|j| j.id.as_str()).collect();
            let mut fresh = esp_core::load_jobs().unwrap_or_default();
            for j in &mut fresh {
                if ids.contains(&j.id.as_str()) {
                    j.last_run = Some(now);
                }
            }
            let _ = esp_core::save_jobs(&fresh);
            syncs::refresh(&ui);
        }

        let backups = esp_core::load_backups().unwrap_or_default();
        let bdue: Vec<_> = backups
            .iter()
            .filter(|b| b.schedule.is_due_at(b.at, b.last_run, now))
            .cloned()
            .collect();
        if !bdue.is_empty() {
            for b in &bdue {
                backups::run_detached(&ui, b);
            }
            let ids: Vec<&str> = bdue.iter().map(|b| b.id.as_str()).collect();
            let mut fresh = esp_core::load_backups().unwrap_or_default();
            for b in &mut fresh {
                if ids.contains(&b.id.as_str()) {
                    b.last_run = Some(now);
                }
            }
            let _ = esp_core::save_backups(&fresh);
            backups::refresh(&ui);
        }

        glib::ControlFlow::Continue
    });
}

struct PageDef {
    name: &'static str,
    title: &'static str,
    icon: &'static str,
}

const PAGES: &[PageDef] = &[
    PageDef {
        name: "overview",
        title: "Overview",
        icon: "view-grid-symbolic",
    },
    PageDef {
        name: "remotes",
        title: "Remotes",
        icon: "network-server-symbolic",
    },
    PageDef {
        name: "syncs",
        title: "Syncs",
        icon: "emblem-synchronizing-symbolic",
    },
    PageDef {
        name: "mounts",
        title: "Mounts",
        icon: "folder-symbolic",
    },
    PageDef {
        name: "backups",
        title: "Backups",
        icon: "drive-harddisk-symbolic",
    },
    PageDef {
        name: "activity",
        title: "Activity",
        icon: "document-open-recent-symbolic",
    },
];

fn build_ui(app: &adw::Application) {
    let providers = esp_core::list_providers().unwrap_or_default();
    let node_count = Rc::new(Cell::new(6usize));

    let stack = adw::ViewStack::new();
    let ov = overview_page(&node_count);
    let overview = ov.page.clone();
    let (remotes_widget, remotes_list) = remotes_page();
    let (syncs_widget, syncs_list, new_sync_btn) = syncs::page();
    let (mounts_widget, mounts_list) = mounts::page();
    let (backups_widget, backups_list, new_backup_btn) = backups::page();
    let (activity_widget, activity_speed, activity_list) = activity::page();
    stack.add_named(&overview, Some("overview"));
    stack.add_named(&remotes_widget, Some("remotes"));
    stack.add_named(&syncs_widget, Some("syncs"));
    stack.add_named(&mounts_widget, Some("mounts"));
    stack.add_named(&backups_widget, Some("backups"));
    stack.add_named(&activity_widget, Some("activity"));

    // Sidebar nav bound to the view stack.
    let nav_list = gtk::ListBox::new();
    nav_list.add_css_class("navigation-sidebar");
    nav_list.set_selection_mode(gtk::SelectionMode::Single);
    for page in PAGES {
        let row = gtk::ListBoxRow::new();
        row.add_css_class("nav");
        let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        hbox.set_margin_top(10);
        hbox.set_margin_bottom(10);
        hbox.set_margin_start(6);
        hbox.set_margin_end(6);
        hbox.append(&gtk::Image::from_icon_name(page.icon));
        hbox.append(&gtk::Label::new(Some(page.title)));
        row.set_child(Some(&hbox));
        nav_list.append(&row);
    }
    let stack_for_nav = stack.clone();
    nav_list.connect_row_selected(move |_, row| {
        if let Some(row) = row {
            if let Some(page) = PAGES.get(row.index() as usize) {
                stack_for_nav.set_visible_child_name(page.name);
            }
        }
    });
    let start_idx = aid("ESP_PAGE")
        .and_then(|name| PAGES.iter().position(|p| p.name == name))
        .unwrap_or(0) as i32;
    nav_list.select_row(nav_list.row_at_index(start_idx).as_ref());

    let brand = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let b1 = gtk::Label::new(Some("Oortery"));
    b1.add_css_class("title-4");
    b1.set_xalign(0.0);
    let b2 = gtk::Label::new(Some("mission control for your data"));
    b2.add_css_class("caption");
    b2.add_css_class("dim");
    b2.set_xalign(0.0);
    brand.append(&b1);
    brand.append(&b2);

    let sidebar_header = adw::HeaderBar::new();
    sidebar_header.set_title_widget(Some(&brand));
    sidebar_header.set_show_end_title_buttons(false);
    let sidebar_scroll = gtk::ScrolledWindow::new();
    sidebar_scroll.set_child(Some(&nav_list));
    sidebar_scroll.set_vexpand(true);
    let sidebar = adw::ToolbarView::new();
    sidebar.add_top_bar(&sidebar_header);
    sidebar.set_content(Some(&sidebar_scroll));

    let content_header = adw::HeaderBar::new();
    let add_btn = gtk::Button::from_icon_name("list-add-symbolic");
    add_btn.set_tooltip_text(Some("Add a remote"));
    add_btn.add_css_class("suggested-action");
    content_header.pack_start(&add_btn);
    let title = adw::WindowTitle::new("Overview", "");
    content_header.set_title_widget(Some(&title));

    // Primary menu: the way to About (and the door into Evening Star).
    let menu_btn = gtk::MenuButton::builder()
        .icon_name("open-menu-symbolic")
        .tooltip_text("Menu")
        .build();
    let menu_box = gtk::Box::new(gtk::Orientation::Vertical, 2);
    menu_box.set_margin_top(4);
    menu_box.set_margin_bottom(4);
    menu_box.set_margin_start(4);
    menu_box.set_margin_end(4);
    let settings_item = gtk::Button::with_label("Settings");
    settings_item.add_css_class("flat");
    settings_item.set_halign(gtk::Align::Fill);
    menu_box.append(&settings_item);
    let about_item = gtk::Button::with_label("About Oortery");
    about_item.add_css_class("flat");
    about_item.set_halign(gtk::Align::Fill);
    menu_box.append(&about_item);
    let diag_item = gtk::Button::with_label("Diagnostics");
    diag_item.add_css_class("flat");
    diag_item.set_halign(gtk::Align::Fill);
    menu_box.append(&diag_item);
    let menu_pop = gtk::Popover::new();
    menu_pop.set_child(Some(&menu_box));
    menu_btn.set_popover(Some(&menu_pop));
    content_header.pack_end(&menu_btn);
    let title_for_notify = title.clone();
    stack.connect_visible_child_notify(move |s| {
        if let Some(name) = s.visible_child_name() {
            if let Some(p) = PAGES.iter().find(|p| p.name == name) {
                title_for_notify.set_title(p.title);
            }
        }
    });
    if let Some(name) = stack.visible_child_name() {
        if let Some(p) = PAGES.iter().find(|p| p.name == name) {
            title.set_title(p.title);
        }
    }

    let content = adw::ToolbarView::new();
    content.add_top_bar(&content_header);
    content.set_content(Some(&stack));

    let split = adw::OverlaySplitView::new();
    split.set_sidebar(Some(&sidebar));
    split.set_content(Some(&content));
    split.set_max_sidebar_width(240.0);
    split.set_min_sidebar_width(210.0);

    let toasts = adw::ToastOverlay::new();
    toasts.set_child(Some(&split));

    // Reopen at the size the user left it (defaults are large enough to show the whole UI).
    let ws = esp_core::load_window_state();
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .default_width(ws.width)
        .default_height(ws.height)
        .width_request(360)
        .height_request(480)
        .content(&toasts)
        .build();
    if ws.maximized {
        window.maximize();
    }
    // Remember the geometry on close. default_width/height track the unmaximized size, so a
    // maximized-then-restored session still reopens at a sensible size.
    window.connect_close_request(|w| {
        esp_core::save_window_state(&esp_core::WindowState {
            width: w.default_width(),
            height: w.default_height(),
            maximized: w.is_maximized(),
        });
        glib::Propagation::Proceed
    });

    let ui = Rc::new(Ui {
        window: window.clone(),
        toasts,
        providers,
        remotes_list,
        remotes_count: ov.remotes,
        node_count,
        syncs_list,
        mounts_list,
        backups_list,
        activity_list,
        activity_speed,
        active_label: ov.active,
        health_banner: ov.health_banner,
        health_icon: ov.health_icon,
        health_text: ov.health_text,
        health_sub: ov.health_sub,
        last_backup_label: ov.last_backup,
        coverage_label: ov.coverage,
        overview_onboarding: ov.onboarding,
        overview_tiles: ov.tiles,
        rcd: RefCell::new(None),
        active_syncs: Rc::new(Cell::new(0)),
        run_seq: Cell::new(0),
    });
    // Reveal the app: show the real content and start reading data / scheduled work. Deferred
    // behind the lock screen when an app-lock password is set, so nothing runs while locked.
    let reveal: Rc<dyn Fn()> = {
        let (ui_r, window_r) = (ui.clone(), window.clone());
        Rc::new(move || {
            window_r.set_content(Some(&ui_r.toasts));
            refresh(&ui_r);
            syncs::refresh(&ui_r);
            mounts::refresh(&ui_r);
            backups::refresh(&ui_r);
            activity::start_polling(&ui_r);
            start_scheduler(&ui_r);
            // If the loads above set a damaged settings file aside, tell the user once rather than
            // letting their syncs/backups vanish silently.
            if esp_core::took_corrupt_recovery() {
                ui_r.toast(
                    "A settings file was unreadable and was set aside; some syncs or backups may need to be re-added.",
                );
            }
        })
    };
    if esp_core::lock_enabled() && aid("ESP_NO_LOCK").is_none() {
        let r = reveal.clone();
        let lock = lockscreen::build(Rc::new(move || r()));
        window.set_content(Some(&lock));
    } else {
        reveal();
    }

    {
        let (win, pop) = (window.clone(), menu_pop.clone());
        about_item.connect_clicked(move |_| {
            pop.popdown();
            about::present(&win);
        });
    }
    {
        let (ui_s, pop) = (ui.clone(), menu_pop.clone());
        settings_item.connect_clicked(move |_| {
            pop.popdown();
            prefs::present(&ui_s);
        });
    }
    {
        let (ui_d, pop) = (ui.clone(), menu_pop.clone());
        diag_item.connect_clicked(move |_| {
            pop.popdown();
            diagnostics::present(&ui_d);
        });
    }

    let ui_for_add = ui.clone();
    add_btn.connect_clicked(move |_| add_remote::present(&ui_for_add));
    let ui_for_new_sync = ui.clone();
    new_sync_btn.connect_clicked(move |_| syncs::present_new(&ui_for_new_sync, None));
    let ui_for_new_backup = ui.clone();
    new_backup_btn.connect_clicked(move |_| backups::present_new(&ui_for_new_backup, None));
    // First-run launcher buttons.
    let ui_connect = ui.clone();
    ov.connect_btn
        .connect_clicked(move |_| add_remote::present(&ui_connect));
    let ui_protect = ui.clone();
    ov.protect_btn
        .connect_clicked(move |_| backups::present_new(&ui_protect, None));

    window.present();

    // Headless aid: auto-open the add-remote dialog for screenshots.
    if aid("ESP_DIALOG").is_some() {
        add_remote::present(&ui);
    }
    // Headless aid: auto-open the new-sync dialog for screenshots.
    if aid("ESP_SYNC_DIALOG").is_some() {
        syncs::present_new(&ui, None);
    }
    // Headless aid: auto-open the folder browser at ESP_BROWSE (a local subpath) for screenshots.
    if let Some(start) = aid("ESP_BROWSE") {
        folder_browser::present(&ui, "/".to_string(), start, std::rc::Rc::new(|_| {}));
    }
    // Headless aid: auto-open the About dialog for screenshots.
    if aid("ESP_ABOUT").is_some() {
        about::present(&window);
    }
    // Headless aid: auto-open the Settings dialog for screenshots.
    if aid("ESP_SETTINGS").is_some() {
        prefs::present(&ui);
    }
    // Headless aid: auto-open the new-backup dialog for screenshots.
    if aid("ESP_BACKUP_DIALOG").is_some() {
        backups::present_new(&ui, None);
    }
    // Headless aid: auto-open the edit dialog for the named remote.
    if let Some(name) = aid("ESP_EDIT") {
        add_remote::present_edit(&ui, &name);
    }
    // Headless aid: auto-open the Diagnostics dialog for screenshots.
    if aid("ESP_DIAG").is_some() {
        esp_core::diag::error("Example error for the screenshot: could not reach the backend");
        diagnostics::present(&ui);
    }
}

/// The widgets the Overview exposes for live updates (the health banner and the tiles).
struct OverviewWidgets {
    page: gtk::Widget,
    health_banner: gtk::Box,
    health_icon: gtk::Image,
    health_text: gtk::Label,
    health_sub: gtk::Label,
    last_backup: gtk::Label,
    coverage: gtk::Label,
    remotes: gtk::Label,
    active: gtk::Label,
    onboarding: gtk::Box,
    tiles: gtk::Box,
    connect_btn: gtk::Button,
    protect_btn: gtk::Button,
}

fn overview_page(node_count: &Rc<Cell<usize>>) -> OverviewWidgets {
    let clamp = adw::Clamp::new();
    clamp.set_maximum_size(1000);
    let scroll = gtk::ScrolledWindow::new();
    scroll.set_vexpand(true);

    let col = gtk::Box::new(gtk::Orientation::Vertical, 20);
    col.set_margin_top(24);
    col.set_margin_bottom(24);
    col.set_margin_start(24);
    col.set_margin_end(24);

    // The "are we OK?" banner: the first thing the eye lands on. Colour + icon + named verdict.
    let health_banner = gtk::Box::new(gtk::Orientation::Horizontal, 14);
    health_banner.add_css_class("health-banner");
    let health_icon = gtk::Image::from_icon_name("emblem-ok-symbolic");
    health_icon.set_pixel_size(28);
    let health_texts = gtk::Box::new(gtk::Orientation::Vertical, 1);
    health_texts.set_hexpand(true);
    let health_text = gtk::Label::new(Some("Checking\u{2026}"));
    health_text.add_css_class("title-3");
    health_text.set_xalign(0.0);
    let health_sub = gtk::Label::new(None);
    health_sub.add_css_class("dim");
    health_sub.set_xalign(0.0);
    health_texts.append(&health_text);
    health_texts.append(&health_sub);
    health_banner.append(&health_icon);
    health_banner.append(&health_texts);
    col.append(&health_banner);

    // The living-sky hero: brand and awe, now secondary to the verdict above it.
    let hero = gtk::DrawingArea::new();
    hero.add_css_class("hero");
    hero.set_content_height(300);
    hero.set_hexpand(true);
    let phase = Rc::new(Cell::new(0.0f64));
    hero.set_draw_func(glib::clone!(
        #[strong]
        node_count,
        #[strong]
        phase,
        move |_area, cr, w, h| {
            constellation::draw(cr, w as f64, h as f64, phase.get(), node_count.get());
        }
    ));
    hero.add_tick_callback(move |area, clock| {
        phase.set(clock.frame_time() as f64 / 1_000_000.0);
        area.queue_draw();
        glib::ControlFlow::Continue
    });
    col.append(&hero);

    // First-run: a welcoming two-step launcher instead of empty tiles. Shown only when nothing
    // is set up yet (toggled in update_overview).
    let onboarding = gtk::Box::new(gtk::Orientation::Vertical, 10);
    onboarding.add_css_class("stat-card");
    onboarding.set_visible(false);
    let ob_title = gtk::Label::new(Some("Welcome to Oortery"));
    ob_title.add_css_class("title-3");
    ob_title.set_xalign(0.0);
    let ob_sub = gtk::Label::new(Some(
        "Protect your data in two steps: connect a place, then back up a folder.",
    ));
    ob_sub.add_css_class("dim");
    ob_sub.set_xalign(0.0);
    ob_sub.set_margin_bottom(6);
    let ob_steps = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let connect_btn = gtk::Button::with_label("Connect a cloud");
    connect_btn.add_css_class("suggested-action");
    connect_btn.add_css_class("pill");
    let protect_btn = gtk::Button::with_label("Back up a folder");
    protect_btn.add_css_class("pill");
    ob_steps.append(&connect_btn);
    ob_steps.append(&protect_btn);
    onboarding.append(&ob_title);
    onboarding.append(&ob_sub);
    onboarding.append(&ob_steps);
    col.append(&onboarding);

    // Actionable tiles: freshness, coverage, remotes, and live activity. No vanity counters.
    let tiles = gtk::Box::new(gtk::Orientation::Horizontal, 16);
    tiles.set_homogeneous(true);
    let (last_card, last_backup) = stat_card("never", "LAST BACKUP", true);
    let (cov_card, coverage) = stat_card("0/0", "PROTECTED", false);
    let (rem_card, remotes) = stat_card("0", "REMOTES", false);
    let (act_card, active) = stat_card("0", "RUNNING NOW", false);
    tiles.append(&last_card);
    tiles.append(&cov_card);
    tiles.append(&rem_card);
    tiles.append(&act_card);
    col.append(&tiles);

    scroll.set_child(Some(&col));
    clamp.set_child(Some(&scroll));
    OverviewWidgets {
        page: clamp.upcast(),
        health_banner,
        health_icon,
        health_text,
        health_sub,
        last_backup,
        coverage,
        remotes,
        active,
        onboarding,
        tiles,
        connect_btn,
        protect_btn,
    }
}

fn stat_card(number: &str, caption: &str, accent: bool) -> (gtk::Widget, gtk::Label) {
    let card = gtk::Box::new(gtk::Orientation::Vertical, 4);
    card.add_css_class("card");
    card.add_css_class("stat-card");
    let num = gtk::Label::new(Some(number));
    num.add_css_class("stat-number");
    if accent {
        num.add_css_class("stat-accent");
    }
    num.set_xalign(0.0);
    let cap = gtk::Label::new(Some(caption));
    cap.add_css_class("stat-caption");
    cap.set_xalign(0.0);
    card.append(&num);
    card.append(&cap);
    (card.upcast(), num)
}

fn remotes_page() -> (gtk::Widget, gtk::ListBox) {
    let clamp = adw::Clamp::new();
    clamp.set_maximum_size(820);
    let scroll = gtk::ScrolledWindow::new();
    scroll.set_vexpand(true);
    let col = gtk::Box::new(gtk::Orientation::Vertical, 10);
    col.set_margin_top(24);
    col.set_margin_bottom(24);
    col.set_margin_start(24);
    col.set_margin_end(24);

    let heading = gtk::Label::new(Some("Configured remotes"));
    heading.add_css_class("title-4");
    heading.set_xalign(0.0);
    let sub = gtk::Label::new(Some(
        "Every storage backend you have connected. rclone can reach many more.",
    ));
    sub.add_css_class("dim");
    sub.set_xalign(0.0);
    sub.set_margin_bottom(8);

    let list = gtk::ListBox::new();
    list.add_css_class("boxed-list");
    list.set_selection_mode(gtk::SelectionMode::None);

    col.append(&heading);
    col.append(&sub);
    col.append(&list);
    scroll.set_child(Some(&col));
    clamp.set_child(Some(&scroll));
    (clamp.upcast(), list)
}
