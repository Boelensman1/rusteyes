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

  config = lib.mkIf cfg.enable {
    assertions = common.assertions;

    environment.systemPackages = [ cfg.package ];

    systemd.user.services.rusteyes = {
      description = "RustEyes break reminder";
      wantedBy = [ "graphical-session.target" ];
      partOf = [ "graphical-session.target" ];
      after = [ "graphical-session.target" ];
      path = [ pkgs.systemd ];
      environment = common.serviceEnvironment;
      serviceConfig = {
        Type = "simple";
        ExecStart = lib.getExe cfg.package;
        Restart = "on-failure";
        RestartSec = "5s";
      };
    };
  };
}
