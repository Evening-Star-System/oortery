//! The startup lock screen: when an app-lock password is set, Oortery opens here and reveals
//! nothing (no data, no daemon, no scheduled work) until the correct password is entered.

use adw::prelude::*;
use std::rc::Rc;

/// Build the lock-screen widget. `on_unlock` runs once the correct password is entered.
pub fn build(on_unlock: Rc<dyn Fn()>) -> gtk::Widget {
    let outer = gtk::Box::new(gtk::Orientation::Vertical, 0);
    outer.set_valign(gtk::Align::Center);
    outer.set_halign(gtk::Align::Center);
    outer.set_vexpand(true);
    outer.set_hexpand(true);

    let card = gtk::Box::new(gtk::Orientation::Vertical, 14);
    card.set_halign(gtk::Align::Center);
    card.set_width_request(360);

    let icon = gtk::Image::from_icon_name("app.eveningstar.Oortery");
    icon.set_pixel_size(88);
    let title = gtk::Label::new(Some("Oortery"));
    title.add_css_class("title-1");
    let sub = gtk::Label::new(Some("Enter your password to unlock"));
    sub.add_css_class("dim");
    sub.set_margin_bottom(6);

    let entry = adw::PasswordEntryRow::builder().title("Password").build();
    // Focus the password field as soon as the lock screen appears, so a keyboard or screen-reader
    // user can type immediately instead of hunting for it.
    entry.connect_map(|e| {
        e.grab_focus();
    });
    let list = gtk::ListBox::new();
    list.add_css_class("boxed-list");
    list.set_selection_mode(gtk::SelectionMode::None);
    list.append(&entry);

    let err = gtk::Label::new(None);
    err.add_css_class("error");
    err.set_visible(false);

    let unlock = gtk::Button::with_label("Unlock");
    unlock.add_css_class("suggested-action");
    unlock.add_css_class("pill");
    unlock.set_margin_top(4);

    card.append(&icon);
    card.append(&title);
    card.append(&sub);
    card.append(&list);
    card.append(&err);
    card.append(&unlock);
    outer.append(&card);

    let attempt = Rc::new({
        let (entry, err, on_unlock) = (entry.clone(), err.clone(), on_unlock.clone());
        move || {
            if esp_core::verify_lock(&entry.text()) {
                on_unlock();
            } else {
                err.set_text("Incorrect password");
                err.set_visible(true);
                entry.set_text("");
            }
        }
    });
    let a1 = attempt.clone();
    unlock.connect_clicked(move |_| a1());
    entry.connect_entry_activated(move |_| attempt());

    outer.upcast()
}
