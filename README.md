# ESP

A mission control for all your data. One native Linux app to organize and run your
syncs, backups, and mounts across every cloud and device, in a place that is actually
a pleasure to look at.

ESP sits on top of [rclone](https://rclone.org) (70+ storage backends) with a native
GTK4 + libadwaita interface. The engine layer is deliberately backend-neutral so more
engines (peer-to-peer sync, snapshot backups) can join without changing the UI.

## Why

The existing options are either heavy Electron apps that break on nvidia and look like a
web page in a box, or narrow tools that wrap a handful of providers. ESP is native, fast,
broad (everything rclone can reach), and built to be beautiful and maintainable for the
long haul. Target: Linux, Flatpak, atomic distros first.

## Status

Early. Working today:

- `esp-core` reads your configured rclone remotes and enumerates every available provider.
- The app shell: adaptive sidebar + view stack, a live Overview with a custom-drawn living
  sky (the Evening Star at the center, your clouds orbiting it), and a Remotes page populated
  from your real rclone config.
- Add a remote across every backend rclone supports (config-value backends end to end;
  OAuth sign-in is next).
- Sync jobs: define a source, a destination, and a mode (Copy or Mirror), then run it and
  watch live progress. Jobs run through a single `rclone rcd` the app owns; that daemon is
  the shared rail backups, mounts, and the Activity stream will reuse.

Not built yet: editing/deleting jobs, scheduling, filters, bidirectional sync, backups,
mounts, the activity stream, and Flatpak packaging.

## Build

```
cargo run -p esp
```

Requires GTK4 and libadwaita development libraries and rclone on the system.

## Layout

- `crates/esp-core` - engine-agnostic core (rclone adapter, backend-neutral models).
- `crates/esp` - the GTK4 + libadwaita application.
