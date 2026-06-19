# Resteyes

Resteyes is a small Rust project exploring a minimal cross-platform Safe Eyes
replacement.

## Getting Started

This repository is set up to work through Nix, so a global Rust install is not
required.

```sh
nix develop
make run
```

On Linux/X11, the current daemon loads configuration, initializes the scheduler,
polls X11 activity, shows unmanaged monitor-covering break overlays when a
break is due, blocks keyboard/pointer input while the overlay is visible, shows
remaining break time, and lets the current break request local locking after it
finishes. Tray controls are added in a later increment.

For a short manual X11 break cycle:

```sh
RESTEYES_CONFIG=test-configs/ten-second-break.yaml make run
```

For temporary LAN discovery smoke testing, run this on two machines using the
same config:

```sh
RESTEYES_DISCOVERY_SMOKE=1 RUST_LOG=info RESTEYES_CONFIG=test-configs/sync-discovery.yaml make run
```

This bypasses the platform backend, starts only mDNS/DNS-SD discovery, and logs
authenticated peers it finds. This smoke path should be removed once discovery
is started by the normal authenticated peer transport/runtime code.

## Common Commands

```sh
make run        # Run the app
make fmt        # Format Rust code
make fmt-check  # Check formatting
make lint       # Run Clippy with warnings denied
make test       # Run tests
make check      # Run fmt-check, lint, and test
make build      # Build the app
```

`make` uses Cargo directly when it is available. If Cargo is not on `PATH`, it
falls back to `nix develop --command cargo ...`.

## Codex Hook

The project includes a local Codex hook that runs `make fmt` after Codex edits
files. Open `/hooks` in Codex once, review the hook, and trust it for this
repository.
