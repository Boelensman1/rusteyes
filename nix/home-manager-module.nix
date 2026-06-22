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
            assertion = !pkgs.stdenv.isDarwin || cfg.configFile == null;
            message = "services.rusteyes.configFile is not supported by the Darwin Home Manager module.";
          }
          {
            assertion = !pkgs.stdenv.isDarwin || cfg.syncSharedSecretFile == null;
            message = "services.rusteyes.syncSharedSecretFile is not supported by the Darwin Home Manager module.";
          }
          {
            assertion = !pkgs.stdenv.isDarwin || cfg.logLevel == "warn";
            message = "services.rusteyes.logLevel is not supported by the Darwin Home Manager module.";
          }
          {
            assertion = !pkgs.stdenv.isDarwin || cfg.extraEnvironment == { };
            message = "services.rusteyes.extraEnvironment is not supported by the Darwin Home Manager module.";
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

      (lib.mkIf pkgs.stdenv.isDarwin {
        xdg.configFile."rusteyes/config.yaml".source = common.generatedConfig;
      })
    ]
  );
}
