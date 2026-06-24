{ self }:
{
  config,
  lib,
  pkgs,
  ...
}@moduleArgs:
let
  common = import ./module-common.nix { inherit self; } moduleArgs;
  cfg = config.services.rusteyes;
in
{
  options.services.rusteyes = common.options;

  config = lib.mkIf cfg.enable (
    lib.mkMerge [
      {
        assertions = common.assertions ++ [
          {
            assertion = pkgs.stdenv.isLinux || pkgs.stdenv.isDarwin;
            message = "services.rusteyes currently supports Home Manager on Linux and Darwin.";
          }
          {
            assertion = !pkgs.stdenv.isDarwin || cfg.launchAgent.enable || cfg.configFile == null;
            message = "services.rusteyes.configFile is only supported on Darwin when services.rusteyes.launchAgent.enable is true.";
          }
          {
            assertion = !pkgs.stdenv.isDarwin || cfg.launchAgent.enable || cfg.syncSharedSecretFile == null;
            message = "services.rusteyes.syncSharedSecretFile is only supported on Darwin when services.rusteyes.launchAgent.enable is true.";
          }
          {
            assertion = !pkgs.stdenv.isDarwin || cfg.launchAgent.enable || cfg.logLevel == "warn";
            message = "services.rusteyes.logLevel is only supported on Darwin when services.rusteyes.launchAgent.enable is true.";
          }
          {
            assertion = !pkgs.stdenv.isDarwin || cfg.launchAgent.enable || cfg.extraEnvironment == { };
            message = "services.rusteyes.extraEnvironment is only supported on Darwin when services.rusteyes.launchAgent.enable is true.";
          }
          {
            assertion =
              !(
                pkgs.stdenv.isDarwin
                && cfg.launchAgent.enable
                && (lib.attrByPath [ "startup" "open_at_login" ] false cfg.settings) == true
              );
            message = ''
              services.rusteyes.launchAgent.enable launches RustEyes at login via a
              LaunchAgent, so settings.startup.open_at_login must not also be true
              (the app would start twice). Set open_at_login to false or omit it.
            '';
          }
        ];

        home.packages = [ cfg.package ];
      }

      (lib.mkIf pkgs.stdenv.isLinux {
        systemd.user.services.rusteyes = {
          Unit = {
            Description = "RustEyes break reminder";
            After = [ "graphical-session.target" ];
            PartOf = [ "graphical-session.target" ];
          };

          Service = {
            Type = "simple";
            ExecStart = lib.getExe cfg.package;
            Environment = [ common.pathEnvironment ] ++ common.environmentList;
            Restart = "on-failure";
            RestartSec = "5s";
          };

          Install = {
            WantedBy = [ "graphical-session.target" ];
          };
        };
      })

      (lib.mkIf pkgs.stdenv.isDarwin (
        lib.mkMerge [
          # Keep writing the default-path config for manual / Login Item
          # launches, but only when not using an external configFile.
          (lib.mkIf (cfg.configFile == null) {
            xdg.configFile."rusteyes/config.yaml".source = common.generatedConfig;
          })

          (lib.mkIf cfg.launchAgent.enable {
            launchd.agents.rusteyes = {
              enable = true;
              config = {
                ProgramArguments = [ (lib.getExe cfg.package) ];
                EnvironmentVariables = common.serviceEnvironment;
                RunAtLoad = true;
                # launchd discards stdout/stderr by default, so the app's
                # tracing output (written to the inherited stderr) would vanish
                # and logLevel/RUST_LOG would have no visible effect. Capture
                # both under ~/Library/Logs so logs are inspectable.
                StandardOutPath = "${config.home.homeDirectory}/Library/Logs/rusteyes.out.log";
                StandardErrorPath = "${config.home.homeDirectory}/Library/Logs/rusteyes.err.log";
                # Restart on crash like systemd Restart=on-failure; a clean Quit
                # (exit 0) leaves the agent stopped.
                KeepAlive = {
                  SuccessfulExit = false;
                };
                # GUI / menu-bar app: avoid launchd batch-job throttling.
                ProcessType = "Interactive";
              };
            };
          })
        ]
      ))
    ]
  );
}
