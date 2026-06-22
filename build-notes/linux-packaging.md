# linux-packaging

## Goal

- Prepare RustEyes for Linux installation through Nix.
- Provide an easy user-session systemd service for graphical Linux/X11
  sessions.

## Decisions

- Expose the flake package through `apps.default` so `nix run` starts
  `rusteyes`.
- Keep the Linux service as a systemd user service wanted by
  `graphical-session.target`; RustEyes needs a user X11 session, tray support,
  desktop notifications, and the session bus.
- Add both NixOS and Home Manager modules with the same option shape:
  generated YAML settings by default, an external `configFile` escape hatch,
  `syncSharedSecretFile` for secrets outside the Nix store, `logLevel`, and
  `extraEnvironment`.
- Generate YAML from module settings and pass it with `RUSTEYES_CONFIG` instead
  of relying on per-user default config creation.
- Add `RUSTEYES_SYNC_SHARED_SECRET_FILE` so sync can be enabled from generated
  Nix config without storing `sync.shared_secret` in the Nix store.
- Strip one trailing `\n`, `\r\n`, or `\r` from the secret file to support
  normal secret-file writers while preserving all other whitespace for existing
  validation.
- Include `systemd` in the service `PATH` so the default Linux lock command
  can find `loginctl`.
- Inject `libappindicator-gtk3` into the wrapped binary's `LD_LIBRARY_PATH`
  via `preFixup`/`gappsWrapperArgs`. `tray-icon` (through `libappindicator-sys`)
  `dlopen`s the appindicator library at runtime, so a build-time `buildInputs`
  entry is not enough and `wrapGAppsHook3` does not add it on its own; without
  the wrapper prefix the binary panicked at startup with
  `Failed to load ayatana-appindicator3 or appindicator3 dynamic library`.
  `libappindicator-gtk3` provides `libappindicator3.so.1`, one of the names the
  loader probes.

## Behavior

- `nix run .` runs the packaged `rusteyes` binary.
- NixOS installs `rusteyes` system-wide and registers a global user unit for
  graphical sessions when `services.rusteyes.enable = true`.
- Home Manager installs `rusteyes` for the user and registers the same
  graphical-session user unit when enabled.
- Module `settings` and `configFile` are mutually exclusive when the service is
  enabled.

## Commands

- `make test`
- `nix flake show --json`
- NixOS module eval with generated settings, `syncSharedSecretFile`, and
  `logLevel`
- Home Manager module eval with generated settings and `syncSharedSecretFile`
- `make check`
- `nix build`
- Attempted `nix build .#packages.x86_64-linux.default --no-link`; this
  machine is `aarch64-darwin` and failed with a platform mismatch once it
  needed to build an `x86_64-linux` Rust dependency, so a Linux builder is still
  needed for full Linux package build verification from this host.
- `nix build .#rusteyes` on x86_64-linux builds the package and the wrapped
  `bin/rusteyes` embeds
  `--prefix LD_LIBRARY_PATH : .../libappindicator-gtk3-.../lib`, confirming the
  appindicator library is on the runtime search path.

## Follow-up

- Manual Linux tray, notification, X11 overlay, and input-blocking verification
  still requires a usable graphical X11 session.
