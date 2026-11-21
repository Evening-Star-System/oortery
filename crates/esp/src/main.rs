//! Oortery: a mission control for all your data.
//!
//! A native GTK4 + libadwaita shell over esp-core. Overview with a live data
//! constellation, a Remotes page from the real rclone config, and an add-remote flow
//! that spans every backend rclone can reach.

mod activity;
mod add_remote;
mod backups;
mod constellation;
mod folder_browser;
mod mounts;
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
    pub backups_count: gtk::Label,
    /// The operations daemon, started lazily the first time a job runs.
    pub rcd: RefCell<Option<Rc<esp_core::Rcd>>>,
    /// Number of jobs currently running (drives the Overview stat).
    pub active_syncs: Rc<Cell<usize>>,
    /// Monotonic counter so each run gets a fresh stats group.
    pub run_seq: Cell<u64>,
}

impl Ui {
    pub fn toast(&self, msg: &str) {
        self.toasts.add_toast(adw::Toast::new(msg));
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
            ui.remotes_list.append(&row);
        }
    }
    ui.remotes_count.set_text(&remotes.len().to_string());
    ui.node_count.set(remotes.len().max(6));
}

fn main() -> glib::ExitCode {
    // Oortery always wears its night sky. Clear any inherited GTK_THEME (an AppImage launcher,
    // or a user's shell) before GTK starts, so nothing overrides our forced dark scheme.
    std::env::remove_var("GTK_THEME");

    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_startup(|_| {
        // Oortery lives in a night sky: force the dark scheme so the look is consistent.
        adw::StyleManager::default().set_color_scheme(adw::ColorScheme::ForceDark);
        load_css();
    });
    app.connect_activate(build_ui);
    app.run()
}

fn load_css() {
    let provider = gtk::CssProvider::new();
    provider.load_from_string(
        "
        @define-color accent_color #b9c0ff;
        @define-color accent_bg_color #5d5fef;
        .stat-card { padding: 18px 22px; min-width: 150px; border-radius: 14px;
                     background: alpha(#ffffff, 0.04); border: 1px solid alpha(#ffffff, 0.07); }
        .stat-number { font-size: 2.3rem; font-weight: 800; }
        .stat-caption { opacity: 0.55; font-size: 0.82rem; letter-spacing: 0.04em; }
        .stat-accent { color: @accent_color; }
        .hero { border-radius: 18px; }
        .dim { opacity: 0.6; }
        row.nav:selected { background: alpha(@accent_color, 0.14); }
        .badge { font-size: 0.72rem; font-weight: 700; letter-spacing: 0.03em;
                 padding: 2px 9px; border-radius: 9px; background: alpha(@accent_color, 0.16);
                 color: @accent_color; }
        .badge.danger { background: alpha(@destructive_color, 0.18); color: @destructive_color; }
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
    let provider_count = providers.len();
    let node_count = Rc::new(Cell::new(6usize));

    let stack = adw::ViewStack::new();
    let (overview, remotes_count, active_label, backups_count) =
        overview_page(provider_count, &node_count);
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
    let start_idx = std::env::var("ESP_PAGE")
        .ok()
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

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .default_width(1120)
        .default_height(740)
        .width_request(360)
        .height_request(480)
        .content(&toasts)
        .build();

    let ui = Rc::new(Ui {
        window: window.clone(),
        toasts,
        providers,
        remotes_list,
        remotes_count,
        node_count,
        syncs_list,
        mounts_list,
        backups_list,
        activity_list,
        activity_speed,
        active_label,
        backups_count,
        rcd: RefCell::new(None),
        active_syncs: Rc::new(Cell::new(0)),
        run_seq: Cell::new(0),
    });
    refresh(&ui);
    syncs::refresh(&ui);
    mounts::refresh(&ui);
    backups::refresh(&ui);
    activity::start_polling(&ui);

    let ui_for_add = ui.clone();
    add_btn.connect_clicked(move |_| add_remote::present(&ui_for_add));
    let ui_for_new_sync = ui.clone();
    new_sync_btn.connect_clicked(move |_| syncs::present_new(&ui_for_new_sync, None));
    let ui_for_new_backup = ui.clone();
    new_backup_btn.connect_clicked(move |_| backups::present_new(&ui_for_new_backup, None));

    window.present();

    // Headless aid: auto-open the add-remote dialog for screenshots.
    if std::env::var("ESP_DIALOG").is_ok() {
        add_remote::present(&ui);
    }
    // Headless aid: auto-open the new-sync dialog for screenshots.
    if std::env::var("ESP_SYNC_DIALOG").is_ok() {
        syncs::present_new(&ui, None);
    }
    // Headless aid: auto-open the folder browser at ESP_BROWSE (a local subpath) for screenshots.
    if let Ok(start) = std::env::var("ESP_BROWSE") {
        folder_browser::present(&ui, "/".to_string(), start, std::rc::Rc::new(|_| {}));
    }
    // Headless aid: auto-open the new-backup dialog for screenshots.
    if std::env::var("ESP_BACKUP_DIALOG").is_ok() {
        backups::present_new(&ui, None);
    }
}

fn overview_page(
    providers: usize,
    node_count: &Rc<Cell<usize>>,
) -> (gtk::Widget, gtk::Label, gtk::Label, gtk::Label) {
    let clamp = adw::Clamp::new();
    clamp.set_maximum_size(1000);
    let scroll = gtk::ScrolledWindow::new();
    scroll.set_vexpand(true);

    let col = gtk::Box::new(gtk::Orientation::Vertical, 22);
    col.set_margin_top(24);
    col.set_margin_bottom(24);
    col.set_margin_start(24);
    col.set_margin_end(24);

    let hero = gtk::DrawingArea::new();
    hero.add_css_class("hero");
    hero.set_content_height(340);
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

    let stats = gtk::Box::new(gtk::Orientation::Horizontal, 16);
    stats.set_homogeneous(true);
    let (remotes_card, remotes_count) = stat_card("0", "REMOTES", true);
    stats.append(&remotes_card);
    stats.append(&stat_card(&format!("{providers}"), "PROVIDERS READY", false).0);
    let (active_card, active_label) = stat_card("0", "ACTIVE SYNCS", false);
    stats.append(&active_card);
    let (backups_card, backups_count) = stat_card("0", "BACKUPS", false);
    stats.append(&backups_card);
    col.append(&stats);

    scroll.set_child(Some(&col));
    clamp.set_child(Some(&scroll));
    (clamp.upcast(), remotes_count, active_label, backups_count)
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
