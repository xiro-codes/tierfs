{ config, lib, pkgs, ... }:

let
  cfg = config.services.tierfs;
  inherit (lib) mkIf mkEnableOption mkOption types concatStringsSep mapAttrsToList;
in
{
  options.services.tierfs = {
    enable = mkEnableOption "tierfs FUSE daemon";

    package = mkOption {
      type = types.package;
      description = "The tierfs package to use. (Usually passed from the flake inputs)";
    };

    mountPoint = mkOption {
      type = types.str;
      description = "The destination path where the tierfs merged filesystem will be mounted.";
    };

    logLevel = mkOption {
      type = types.int;
      default = 2;
      description = "Global log level (0 = None, 1 = Normal, 2 = Debug).";
    };

    logFile = mkOption {
      type = types.nullOr types.str;
      default = null;
      description = "Path to log file. If null, logs go to stdout/journald.";
    };

    tierPeriod = mkOption {
      type = types.int;
      default = 1000;
      description = "Tier period in seconds. How often the background thread checks quotas and triggers data migrations.";
    };

    copyBufferSize = mkOption {
      type = types.str;
      default = "1 MiB";
      description = "Copy buffer size for migration reads/writes (e.g. '1 MiB', '4 KiB').";
    };

    runPath = mkOption {
      type = types.str;
      default = "/var/lib/tierfs";
      description = "Run path for the metadata database and internal daemon state.";
    };

    tiers = mkOption {
      type = types.attrsOf (types.submodule {
        options = {
          path = mkOption {
            type = types.str;
            description = "Physical path for this tier (e.g. '/mnt/ssd', '/mnt/hdd').";
          };
          quota = mkOption {
            type = types.str;
            default = "80%";
            description = "Quota threshold for this tier (e.g. '80%', '1 TiB').";
          };
        };
      });
      default = {};
      description = "Storage tiers configuration. E.g. { Tier1 = { path = \"/mnt/fast\"; quota = \"80%\"; }; }";
    };
  };

  config = mkIf cfg.enable {
    environment.etc."tierfs.conf".text = ''
      [Global]
      Log Level = ${toString cfg.logLevel}
      Tier Period = ${toString cfg.tierPeriod}
      Copy Buffer Size = ${cfg.copyBufferSize}
      Run Path = ${cfg.runPath}
      ${lib.optionalString (cfg.logFile != null) "Log File = ${cfg.logFile}"}

      ${concatStringsSep "\n" (mapAttrsToList (name: tier: ''
        [${name}]
        Path = ${tier.path}
        Quota = ${tier.quota}
      '') cfg.tiers)}
    '';

    systemd.services.tierfs = {
      description = "TierFS FUSE Daemon";
      wantedBy = [ "multi-user.target" ];
      after = [ "local-fs.target" ];

      preStart = ''
        mkdir -p ${cfg.mountPoint}
        mkdir -p ${cfg.runPath}
        ${concatStringsSep "\n" (mapAttrsToList (name: tier: ''
          mkdir -p -m 0700 ${tier.path}
          chmod 700 ${tier.path}
        '') cfg.tiers)}
      '';

      serviceConfig = {
        ExecStart = "${cfg.package}/bin/tierfs --verbose --config /etc/tierfs.conf ${cfg.mountPoint}";
        ExecStop = "${pkgs.fuse3}/bin/fusermount3 -u ${cfg.mountPoint}";
        Restart = "on-failure";
        Type = "simple";
      };
    };
  };
}
