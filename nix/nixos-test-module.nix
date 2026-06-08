{ config, lib, pkgs, ... }:

let
  cfg = config.services.tierfs-test;
  inherit (lib) mkIf mkEnableOption mkOption types;
in
{
  imports = [ ./nixos-module.nix ];

  options.services.tierfs-test = {
    enable = mkEnableOption "TierFS test environment using tmpfs storage tiers";

    mountPoint = mkOption {
      type = types.str;
      default = "/mnt/tierfs/merged";
      description = "The destination path where the tierfs merged filesystem will be mounted.";
    };

    runPath = mkOption {
      type = types.str;
      default = "/var/lib/tierfs-test";
      description = "Run path for the metadata database and internal daemon state.";
    };
  };

  config = mkIf cfg.enable {
    fileSystems."/mnt/tierfs/tier1" = {
      device = "tmpfs";
      fsType = "tmpfs";
      options = [ "size=1G" "mode=0755" ];
    };

    fileSystems."/mnt/tierfs/tier2" = {
      device = "tmpfs";
      fsType = "tmpfs";
      options = [ "size=2G" "mode=0755" ];
    };

    fileSystems."/mnt/tierfs/tier3" = {
      device = "tmpfs";
      fsType = "tmpfs";
      options = [ "size=4G" "mode=0755" ];
    };

    services.tierfs = {
      enable = true;
      mountPoint = cfg.mountPoint;
      runPath = cfg.runPath;
      tiers = {
        Tier1 = {
          path = "/mnt/tierfs/tier1";
          quota = "80%";
        };
        Tier2 = {
          path = "/mnt/tierfs/tier2";
          quota = "80%";
        };
        Tier3 = {
          path = "/mnt/tierfs/tier3";
          quota = "80%";
        };
      };
    };
  };
}
