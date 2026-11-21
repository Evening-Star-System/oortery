//! The About dialog: who made Oortery, the license, and the way in to Evening Star.

use adw::prelude::*;

/// Invite link into Evening Star (the wider ESS community and platform).
const INVITE_URL: &str = "https://go.eveningstar.app/oortery";

/// Present the About dialog over the main window.
pub fn present(window: &adw::ApplicationWindow) {
    let about = adw::AboutDialog::builder()
        .application_name("Oortery")
        .application_icon("app.eveningstar.Oortery")
        .developer_name("Evening Star Productions")
        .version(env!("CARGO_PKG_VERSION"))
        .license_type(gtk::License::Agpl30)
        .website("https://eveningstar.app")
        .copyright("\u{00a9} Evening Star Productions")
        .comments(
            "Mission control for all your data: sync, back up, and mount every cloud. \
             Open source, account-free, and your keys and your backends stay yours.",
        )
        .build();
    // The way in to the wider Evening Star community.
    about.add_link("Join Evening Star", INVITE_URL);
    about.present(Some(window));
}
