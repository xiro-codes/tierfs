{
  description = "A standard Rust CLI application";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/15f4ee454b1dce334612fa6843b3e05cf546efab";
    flake-parts = {
      url = "github:hercules-ci/flake-parts/71a3a77326609675e9f8b51084cf23d5d1945899";
      inputs.nixpkgs-lib.follows = "nixpkgs";
    };
    rust-overlay = {
      url = "github:oxalica/rust-overlay/366ea19e0e55b768f74b7a0b2a20f847e7ae828d";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    autotier-src = {
      url = "github:45Drives/autotier";
      flake = false;
    };
  };

  outputs =
    inputs@{ flake-parts, rust-overlay, ... }:
    let
      inherit (flake-parts) lib;
    in
    lib.mkFlake { inherit inputs; } {
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];
      flake = {
        nixosModules.default = import ./nixos-module.nix;
      };
      perSystem =
        { pkgs, system, ... }:
        let
          overlays = [ (import rust-overlay) ];
          pkgsWithRust = import inputs.nixpkgs { inherit system overlays; };

          rustToolchain = pkgsWithRust.rust-bin.stable.latest.default.override {
            extensions = [
              "rust-src"
              "rust-analyzer"
            ];
          };

          tierfsPkg = pkgsWithRust.rustPlatform.buildRustPackage {
            pname = "tierfs";
            version = "0.1.0";
            src = ./.;
            cargoLock = {
              lockFile = ./Cargo.lock;
            };
          };

          autotierCppPkg = pkgsWithRust.autotier.overrideAttrs (old: {
            src = inputs.autotier-src;
            doCheck = false;
          });
        in
        {
          formatter = pkgs.nixfmt;
          
          packages.default = tierfsPkg;
          packages.autotier-cpp = autotierCppPkg;

          packages.benchmark = pkgsWithRust.writeShellApplication {
            name = "tierfs-benchmark";
            runtimeInputs = with pkgsWithRust; [ fio just tierfsPkg autotierCppPkg ];
            text = ''
              GREEN='\033[0;32m'
              RED='\033[0;31m'
              YELLOW='\033[1;33m'
              NC='\033[0m'

              MOUNT_POINT="test_mounts/merged"
              TEST_FILE="$MOUNT_POINT/fio_benchmark_file"
              CONFIG_FILE="test_images/tierfs.conf"

              echo -e "''${GREEN}===========================================''${NC}"
              echo -e "''${GREEN}=== Phase 1: Original C++ Autotier      ===''${NC}"
              echo -e "''${GREEN}===========================================''${NC}"

              echo -e "\n''${YELLOW}[1/3] Setting up simulated tiers (Requires Sudo)...''${NC}"
              just setup-test

              echo -e "\n''${YELLOW}[2/3] Mounting Autotier (C++)...''${NC}"
              sudo ${autotierCppPkg}/bin/autotierfs -c "$CONFIG_FILE" "$MOUNT_POINT" -o allow_other

              sleep 2

              if ! mountpoint -q "$MOUNT_POINT"; then
                  echo -e "''${RED}Error: Failed to mount autotierfs at $MOUNT_POINT''${NC}"
                  just cleanup-test
                  exit 1
              fi

              echo -e "\n''${GREEN}Mount successful! Starting FIO benchmarks...''${NC}"
              echo "Note: The first writes will land on Tier 1 (SSD)."

              echo -e "\n''${YELLOW}>>> Running Sequential Write Test...''${NC}"
              sudo fio --name=seqwrite --filename="$TEST_FILE" --size=200M --rw=write --bs=1M --direct=1 --numjobs=1 --time_based=0 --runtime=60 --group_reporting

              echo -e "\n''${YELLOW}>>> Running Sequential Read Test...''${NC}"
              sudo fio --name=seqread --filename="$TEST_FILE" --size=200M --rw=read --bs=1M --direct=1 --numjobs=1 --time_based=0 --runtime=60 --group_reporting

              echo -e "\n''${YELLOW}>>> Running Random Write Test...''${NC}"
              sudo fio --name=randwrite --filename="$TEST_FILE" --size=50M --rw=randwrite --bs=4k --direct=1 --numjobs=1 --time_based=0 --runtime=60 --group_reporting

              echo -e "\n''${YELLOW}>>> Running Random Read Test...''${NC}"
              sudo fio --name=randread --filename="$TEST_FILE" --size=50M --rw=randread --bs=4k --direct=1 --numjobs=1 --time_based=0 --runtime=60 --group_reporting

              echo -e "\n''${YELLOW}[3/3] Cleaning up environment...''${NC}"
              just cleanup-test

              echo -e "''${GREEN}===========================================''${NC}"
              echo -e "''${GREEN}=== Phase 2: Rust TierFS                ===''${NC}"
              echo -e "''${GREEN}===========================================''${NC}"

              echo -e "\n''${YELLOW}[1/3] Setting up simulated tiers (Requires Sudo)...''${NC}"
              just setup-test

              echo -e "\n''${YELLOW}[2/3] Mounting TierFS (Rust)...''${NC}"
              sudo ${tierfsPkg}/bin/tierfs --config "$CONFIG_FILE" "$MOUNT_POINT"

              sleep 2

              if ! mountpoint -q "$MOUNT_POINT"; then
                  echo -e "''${RED}Error: Failed to mount tierfs at $MOUNT_POINT''${NC}"
                  just cleanup-test
                  exit 1
              fi

              echo -e "\n''${GREEN}Mount successful! Starting FIO benchmarks...''${NC}"

              echo -e "\n''${YELLOW}>>> Running Sequential Write Test...''${NC}"
              sudo fio --name=seqwrite --filename="$TEST_FILE" --size=200M --rw=write --bs=1M --direct=1 --numjobs=1 --time_based=0 --runtime=60 --group_reporting

              echo -e "\n''${YELLOW}>>> Running Sequential Read Test...''${NC}"
              sudo fio --name=seqread --filename="$TEST_FILE" --size=200M --rw=read --bs=1M --direct=1 --numjobs=1 --time_based=0 --runtime=60 --group_reporting

              echo -e "\n''${YELLOW}>>> Running Random Write Test...''${NC}"
              sudo fio --name=randwrite --filename="$TEST_FILE" --size=50M --rw=randwrite --bs=4k --direct=1 --numjobs=1 --time_based=0 --runtime=60 --group_reporting

              echo -e "\n''${YELLOW}>>> Running Random Read Test...''${NC}"
              sudo fio --name=randread --filename="$TEST_FILE" --size=50M --rw=randread --bs=4k --direct=1 --numjobs=1 --time_based=0 --runtime=60 --group_reporting

              echo -e "\n''${YELLOW}[3/3] Cleaning up environment...''${NC}"
              just cleanup-test

              echo -e "\n''${GREEN}=== Benchmarks Completed ===''${NC}"
            '';
          };

          devShells.default = pkgsWithRust.mkShell {
            nativeBuildInputs = with pkgsWithRust; [
              rustToolchain
              pkg-config
              just
              fio
            ];
            buildInputs = with pkgsWithRust; [ fuse3 ];

            shellHook = ''
              echo "🦀 Rust CLI Dev Environment Loaded"
              echo "Rust version: \$(rustc --version)"
              if [ ! -f Cargo.toml ]; then
                echo "=> No Cargo.toml found. Run 'cargo init' to set up a new project."
              fi
              echo "Run 'direnv allow' to automatically load this environment."
            '';
          };
        };
    };
}
