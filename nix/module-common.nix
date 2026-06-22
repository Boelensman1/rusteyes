{ self }:
{
  config,
  lib,
  pkgs,
  ...
}:
let
  inherit (lib)
    literalExpression
    mkEnableOption
    mkOption
    optionalAttrs
    types
    ;

  cfg = config.services.rusteyes;
  yamlFormat = pkgs.formats.yaml { };
  defaultPackage = self.packages.${pkgs.stdenv.hostPlatform.system}.default;
  generatedConfig = yamlFormat.generate "rusteyes-config.yaml" cfg.settings;
  configPath = if cfg.configFile == null then generatedConfig else cfg.configFile;
  serviceEnvironment =
    {
      RUSTEYES_CONFIG = toString configPath;
      RUST_LOG = cfg.logLevel;
    }
    // optionalAttrs (cfg.syncSharedSecretFile != null) {
      RUSTEYES_SYNC_SHARED_SECRET_FILE = cfg.syncSharedSecretFile;
    }
    // cfg.extraEnvironment;
in
{
  options = {
    enable = mkEnableOption "RustEyes";

    package = mkOption {
      type = types.package;
      default = defaultPackage;
      defaultText = literalExpression "self.packages.\${pkgs.stdenv.hostPlatform.system}.default";
      description = "RustEyes package to install and run.";
    };

    settings = mkOption {
      type = yamlFormat.type;
      default = { };
      example = literalExpression ''
        {
          breaks.after_active = "20m";
          sync.enabled = true;
        }
      '';
      description = ''
        RustEyes settings rendered to YAML. Linux services receive this through
        RUSTEYES_CONFIG; Darwin Home Manager installs it at RustEyes' default
        config path. Do not put sync.shared_secret here; use
        syncSharedSecretFile instead.
      '';
    };

    configFile = mkOption {
      type = types.nullOr (types.oneOf [
        types.path
        types.str
      ]);
      default = null;
      example = "/etc/rusteyes/config.yaml";
      description = ''
        External RustEyes YAML config file. When set, settings must be empty.
      '';
    };

    syncSharedSecretFile = mkOption {
      type = types.nullOr types.str;
      default = null;
      example = "/run/secrets/rusteyes-sync-secret";
      description = ''
        Runtime path to a file containing the sync shared secret. The value is
        passed with RUSTEYES_SYNC_SHARED_SECRET_FILE and is not copied into the
        Nix store by this module.
      '';
    };

    logLevel = mkOption {
      type = types.str;
      default = "warn";
      example = "info";
      description = "RUST_LOG value for the RustEyes service.";
    };

    extraEnvironment = mkOption {
      type = types.attrsOf types.str;
      default = { };
      example = literalExpression ''
        {
          DISPLAY = ":0";
        }
      '';
      description = "Additional environment variables for the RustEyes service.";
    };
  };

  assertions = [
    {
      assertion = cfg.configFile == null || cfg.settings == { };
      message = "services.rusteyes.settings cannot be used when services.rusteyes.configFile is set.";
    }
  ];

  inherit generatedConfig;
  inherit serviceEnvironment;

  pathEnvironment = "PATH=${lib.makeBinPath [ pkgs.systemd ]}";

  environmentList = lib.mapAttrsToList (
    name: value: "${name}=${toString value}"
  ) serviceEnvironment;
}
