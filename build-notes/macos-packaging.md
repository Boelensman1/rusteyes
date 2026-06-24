# macos-packaging

## Goal

- Package RustEyes for macOS through Nix as an app bundle with the bundled
  Swift helper.
- Extend Home Manager support for Darwin without adding a LaunchAgent.

## Decisions

- Darwin `packages.default` now points at a `RustEyes.app` bundle package.
- The raw Rust binary package remains available as `packages.<system>.rusteyes`.
- Darwin also exposes `macos-app` and `macos-helper` package attributes.
- The Swift helper is built by Nix with `swift` and `swiftpm`; no
  `swiftpm2nix` dependency metadata is needed because the helper package has no
  SwiftPM dependencies.
- The Nix app bundle installs `RustEyes.app` under `$out/Applications` and a
  `bin/rusteyes` wrapper that best-effort registers the bundle with Launch
  Services before execing the bundled app executable.
- Nix signs the app's Mach-O executable and helper with ad-hoc signatures using
  `darwin.sigtool`; the main executable is signed with identifier
  `dev.rusteyes.RustEyes`, and the helper with
  `dev.rusteyes.RustEyes.helper`. The bundle directory itself is not signed
  because Nix's sigtool signs binaries, not `.app` directories.
- Darwin Home Manager is install/config only by default. It installs the
  selected package and writes generated settings to
  `~/.config/rusteyes/config.yaml`.
- macOS startup defaults to RustEyes' `startup.open_at_login` config and the
  app's Login Item registration path. An opt-in `launchAgent.enable` LaunchAgent
  was added later (see `macos-launchagent.md`) as the alternative startup path.
- With the LaunchAgent disabled, Darwin Home Manager rejects
  service-environment-only options such as `configFile`, `syncSharedSecretFile`,
  `logLevel`, and `extraEnvironment` instead of ignoring them. Enabling the
  LaunchAgent unlocks them, and asserts that `startup.open_at_login` is not also
  set so the app is not launched twice.

## Behavior

- `nix run .` on Darwin starts the executable inside the packaged
  `RustEyes.app` bundle after registering that bundle with Launch Services.
- Home Manager on Linux keeps installing the graphical-session systemd user
  service.
- Home Manager on Darwin installs/configures RustEyes but does not create
  systemd or launchd services.
- Home Manager's built-in Darwin app linking/copying modules can expose the
  bundle in `~/Applications/Home Manager Apps`.

## Commands

- `nix eval --raw .#packages.aarch64-darwin.default.name`
- `nix eval --json .#packages.aarch64-darwin --apply builtins.attrNames`
- `nix eval --raw .#packages.aarch64-linux.default.name`
- `nix build .#packages.aarch64-darwin.default --no-link --print-out-paths`
- `find /nix/store/...-rusteyes-0.1.0 -maxdepth 5 -type f -o -type l`
- `plutil -lint /nix/store/...-rusteyes-0.1.0/Applications/RustEyes.app/Contents/Info.plist`
- `codesign -dv /nix/store/...-rusteyes-0.1.0/Applications/RustEyes.app/Contents/MacOS/rusteyes`
- `codesign -dv /nix/store/...-rusteyes-0.1.0/Applications/RustEyes.app/Contents/Resources/rusteyes-macos-helper`
- `sed -n '1,80p' /nix/store/...-rusteyes-0.1.0/bin/rusteyes`
- Home Manager module evals for Linux systemd service behavior, Darwin
  install/config behavior, and Darwin unsupported-option assertions.
- `nix flake show --json`
- `make check`

## Follow-up

- Manual verification of the Nix-installed macOS app is still pending after
  granting the app the required Accessibility/Input Monitoring permissions.
