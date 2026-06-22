# RustEyes

RustEyes is a small Rust project exploring a minimal cross-platform Safe Eyes
replacement.

## Getting Started

This repository is set up to work through Nix, so a global Rust install is not
required.

```sh
nix develop
make run
```

You can also run or install the flake package directly:

```sh
nix run .
nix profile install .
```

On Linux/X11, the current daemon loads configuration, initializes the scheduler,
polls X11 activity, shows unmanaged monitor-covering break overlays when a
break is due, blocks keyboard/pointer input while the overlay is visible, shows
remaining break time, lets the current break request local locking after it
finishes, sends pre-break notifications, and exposes tray controls for manual
breaks, disable actions, and quit.

For a short manual X11 break cycle:

```sh
RUSTEYES_CONFIG=test-configs/ten-second-break.yaml make run
```

For temporary LAN discovery smoke testing, run this on two machines using the
same config:

```sh
RUSTEYES_DISCOVERY_SMOKE=1 RUST_LOG=info RUSTEYES_CONFIG=test-configs/sync-discovery.yaml make run
```

This bypasses the platform backend, starts only mDNS/DNS-SD discovery, and logs
authenticated peers it finds. This smoke path should be removed once discovery
is started by the normal authenticated peer transport/runtime code.

## NixOS and Home Manager

The flake exposes a Linux package and a macOS `RustEyes.app` bundle. On macOS,
`nix run .` and `nix profile install .` use the app bundle package so RustEyes
can find its bundled helper and use its app identity for notifications.

RustEyes is a user-session application. On Linux, the modules install a
`systemd --user` service wanted by `graphical-session.target` instead of a
system daemon. On macOS, the Home Manager module is install/config only:
launch RustEyes manually, or set `startup.open_at_login = true` in the config
and launch the app once so RustEyes can register its Login Item.

NixOS:

```nix
{
  inputs.rusteyes.url = "path:/path/to/rusteyes";

  outputs = { nixpkgs, rusteyes, ... }: {
    nixosConfigurations.my-host = nixpkgs.lib.nixosSystem {
      system = "x86_64-linux";
      modules = [
        rusteyes.nixosModules.default
        {
          services.rusteyes = {
            enable = true;
            settings = {
              breaks.after_active = "20m";
              sync.enabled = true;
            };
            syncSharedSecretFile = "/run/secrets/rusteyes-sync-secret";
          };
        }
      ];
    };
  };
}
```

Home Manager:

```nix
{
  imports = [ rusteyes.homeManagerModules.default ];

  services.rusteyes = {
    enable = true;
    settings.breaks.after_active = "20m";
    syncSharedSecretFile = "/run/secrets/rusteyes-sync-secret";
  };
}
```

The Linux modules generate YAML from `services.rusteyes.settings` and pass it
through `RUSTEYES_CONFIG`. Use `services.rusteyes.configFile` instead when you
want to manage YAML yourself. Sync secrets should not be placed in generated
Nix settings; use `syncSharedSecretFile`, which maps to
`RUSTEYES_SYNC_SHARED_SECRET_FILE` at runtime.

On macOS, Home Manager writes generated settings to
`~/.config/rusteyes/config.yaml`. Home Manager's Darwin app handling can expose
the bundle in `~/Applications/Home Manager Apps`:

```nix
{
  imports = [ rusteyes.homeManagerModules.default ];

  services.rusteyes = {
    enable = true;
    settings = {
      breaks.after_active = "20m";
      startup.open_at_login = true;
    };
  };

  targets.darwin.copyApps.enable = true;
}
```

By default the Darwin module is install/config only: it installs the app and
writes `~/.config/rusteyes/config.yaml`, and startup is left to the app's own
Login Item (`settings.startup.open_at_login`). In that mode the service-only
options `configFile`, `syncSharedSecretFile`, `logLevel`, and `extraEnvironment`
are not supported, because there is no service to inject their environment into.

Set `services.rusteyes.launchAgent.enable = true` to manage a macOS LaunchAgent
that runs RustEyes at login and injects the service environment (the same way
the Linux systemd user service does). Enabling it unlocks `configFile`,
`syncSharedSecretFile`, `logLevel`, and `extraEnvironment` on Darwin — this is
how the sync shared secret reaches the app on macOS. The LaunchAgent runs the
app at login itself, so `settings.startup.open_at_login` must not also be true
(the build fails otherwise to avoid launching the app twice).

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
