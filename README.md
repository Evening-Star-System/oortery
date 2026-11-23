# Oortery

Mission control for all your data. One native Linux app to organize and run your syncs, backups,
mounts, and device-to-device sync across every cloud, with no account to sign up for and nothing
locked in.

Oortery sits on top of [rclone](https://rclone.org) (70+ storage backends),
[restic](https://restic.net) (versioned encrypted backups), and [Syncthing](https://syncthing.net)
(peer-to-peer sync), under a native GTK4 + libadwaita interface. The engine layer is backend-neutral,
so the app presents one calm surface over three proven tools.

## Why

Your files should be yours: reachable, recoverable, and never held hostage by an account. The usual
options are heavy Electron wrappers that look like a web page in a box, or narrow tools tied to a
handful of providers. Oortery is native, fast, broad (everything rclone can reach), and account-free
by design. You bring your own storage; Oortery just drives it, well.

## What it does

- **Connect anything.** Dozens of storage providers: one-tap browser sign-in for the big clouds
  (Google Drive, Dropbox, OneDrive, and more), short forms for the rest, and simple presets for
  Amazon S3, Cloudflare R2, Backblaze B2, and any S3-compatible store.
- **Sync.** Copy (which never deletes) or Mirror a folder to a cloud, with live progress, optional
  schedules, and a one-tap reconnect when a token expires.
- **Back up.** Versioned, encrypted, deduplicated snapshots via restic, to a local folder or any
  cloud, with restore and an integrity check. The repository password lives in your OS keyring.
- **Mount.** Open a connected cloud as a folder on your computer, with nothing downloaded up front.
- **Peers.** Sync straight between your own devices over Syncthing, with no cloud and no account in
  the middle.
- **Overview.** A status-first home that tells you at a glance whether your data is protected.

Everything runs through a single `rclone rcd` the app owns, the shared engine rail. Account-free,
offline-first, and open: nothing here keeps you, and you can always leave with your data intact.

## Install

Download the AppImage from the
[latest release](https://github.com/Evening-Star-System/oortery/releases/latest), make it
executable, and run it. It is self-contained (it bundles rclone, restic, and Syncthing), so there is
nothing else to install. It needs a reasonably current Linux (glibc 2.39 or newer, for example Ubuntu
24.04+); the app tells you clearly if your system is older. Flatpak packaging is planned.

## Build

```
cargo run -p esp
```

Requires the GTK4 and libadwaita development libraries, plus `rclone` and `restic` on the system (the
release AppImage bundles these for end users).

## Layout

- `crates/esp-core`: the engine-agnostic core (rclone, restic, and Syncthing adapters; backend-neutral models).
- `crates/esp`: the GTK4 + libadwaita application.

## License

[AGPL-3.0-or-later](LICENSE), dual-licensed for commercial use. See [CONTRIBUTING.md](CONTRIBUTING.md).
