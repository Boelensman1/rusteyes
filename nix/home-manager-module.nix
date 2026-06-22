{ self }:
{
  config,
  lib,
  ...
}@moduleArgs:
let
  common = import ./module-common.nix { inherit self; } moduleArgs;
  cfg = config.services.rusteyes;
in
{
  options.services.rusteyes = common.options;

  config = lib.mkIf cfg.enable {
    assertions = common.assertions;

    home.packages = [ cfg.package ];

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
  };
}
