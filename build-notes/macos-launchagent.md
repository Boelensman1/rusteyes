# macos-launchagent

## Goal

- Add an opt-in macOS LaunchAgent to the Home Manager module so the
  service-environment options (notably the sync shared secret) can reach the app
  on macOS, mirroring the Linux systemd user service.

## Problem

- The Darwin Home Manager module was install/config only and created no service.
  Because the service-environment-only options (`configFile`,
  `syncSharedSecretFile`, `logLevel`, `extraEnvironment`) are delivered through a
  service's environment, the module hard-failed when they were set on Darwin.
- In particular there was no Nix-store-safe way to deliver the sync shared secret
  to the app on macOS: `syncSharedSecretFile` becomes
  `RUSTEYES_SYNC_SHARED_SECRET_FILE` in the Linux systemd unit's `Environment=`,
  but macOS had no equivalent injection point.

## Changes

- Added `services.rusteyes.launchAgent.enable` (in `nix/module-common.nix`,
  default `false`) — macOS Home Manager only; inert on Linux.
- Darwin Home Manager branch (`nix/home-manager-module.nix`) now `mkMerge`s:
  - the existing `xdg.configFile."rusteyes/config.yaml"` write, restricted to
    `cfg.configFile == null` (so an external configFile no longer also writes an
    empty generated config to the default path); and
  - when `launchAgent.enable`, a `launchd.agents.rusteyes` agent whose `config`
    sets `ProgramArguments = [ (lib.getExe cfg.package) ]`,
    `EnvironmentVariables = common.serviceEnvironment`, `RunAtLoad = true`,
    `KeepAlive = { SuccessfulExit = false; }`, `ProcessType = "Interactive"`,
    and `StandardOutPath`/`StandardErrorPath` under
    `${config.home.homeDirectory}/Library/Logs/rusteyes.{out,err}.log`.
- Relaxed the four Darwin assertions to fire only when
  `!cfg.launchAgent.enable` — enabling the agent unlocks `configFile`,
  `syncSharedSecretFile`, `logLevel`, and `extraEnvironment` on Darwin.
- Added a hard assertion that fails when `launchAgent.enable` is true and
  `settings.startup.open_at_login` is also true, to prevent the app from being
  launched twice at login (LaunchAgent `RunAtLoad` plus the app's Login Item).

## Decisions

- Opt-in (default off) so the existing install/config-only behavior and the
  Login Item startup path are preserved; the LaunchAgent is the alternative
  startup mechanism, not an addition on top of the Login Item.
- Reuse `common.serviceEnvironment` verbatim so Linux and macOS inject the same
  environment; no new env plumbing.
- `KeepAlive = { SuccessfulExit = false; }` rather than `true`: restart on a
  crash (non-zero exit) like systemd `Restart=on-failure`, but a clean Quit
  (exit 0) leaves the agent stopped so the menu-bar Quit action works.
- `ProcessType = "Interactive"` because RustEyes is a GUI/menu-bar app, avoiding
  launchd background-batch throttling.
- Set `StandardOutPath`/`StandardErrorPath` because launchd otherwise wires the
  agent's stdout/stderr to `/dev/null`. The app writes tracing output to
  `/dev/stderr` (`src/main.rs`), so without these the logs vanish and the
  `logLevel`/`RUST_LOG` option has no observable effect on macOS. Paths are
  hardcoded under `~/Library/Logs` (the macOS convention) rather than exposed as
  options to keep the module surface small; users can still tail those files.
- `lib.getExe cfg.package` resolves on Darwin to the `bin/rusteyes` wrapper that
  execs the bundled app executable, so the bundled helper is still discovered
  relative to the executable — no `PATH`/`HELPER_PATH_ENV` plumbing is needed.
- `pathEnvironment` (which references the Linux-only `pkgs.systemd`) is not used
  in the Darwin branch.
- The double-launch guard is a hard assertion (chosen over a soft warning) so a
  misconfiguration cannot silently launch the app twice. It can only inspect
  inline `settings`; an external `configFile` is opaque, which is acceptable.

## Commands

- `nix flake show --json`
- Home Manager module evals for the Darwin LaunchAgent config, relaxed
  assertions when enabled, retained assertions when disabled, the double-launch
  assertion, and the unchanged Linux systemd service.
- `make check`

## Follow-up

- Manual macOS verification is pending: apply the Home Manager config on a Mac,
  confirm `~/Library/LaunchAgents/org.nix-community.home.rusteyes.plist` exists
  and `launchctl list | grep rusteyes` shows it loaded, and confirm the app
  starts at login and picks up the synced secret.
