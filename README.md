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
polls X11 activity, and prints temporary activity diagnostics until interrupted.
Display overlays, input blocking, and break UI are added in later increments.

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
