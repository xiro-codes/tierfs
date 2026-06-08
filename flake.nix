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
        nixosModules.default = import ./nix/nixos-module.nix;
        nixosModules.test = import ./nix/nixos-test-module.nix;
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
            runtimeInputs = with pkgsWithRust; [
              fio
              just
              tierfsPkg
              autotierCppPkg
            ];
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
              #just cleanup-test

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

          checks = pkgs.lib.optionalAttrs pkgs.stdenv.isLinux {
            tierfs-test = pkgs.testers.nixosTest {
              name = "tierfs-test";
              nodes.machine =
                { pkgs, ... }:
                {
                  imports = [
                    ./nix/nixos-test-module.nix
                  ];
                  services.tierfs-test.enable = true;
                  services.tierfs.package = tierfsPkg;

                  environment.systemPackages = with pkgs; [
                    fio
                    autotierCppPkg
                  ];

                  virtualisation.fileSystems."/mnt/tierfs/tier1" = {
                    device = "tmpfs";
                    fsType = "tmpfs";
                    options = [ "size=1G" "mode=0755" ];
                  };
                  virtualisation.fileSystems."/mnt/tierfs/tier2" = {
                    device = "tmpfs";
                    fsType = "tmpfs";
                    options = [ "size=2G" "mode=0755" ];
                  };
                  virtualisation.fileSystems."/mnt/tierfs/tier3" = {
                    device = "tmpfs";
                    fsType = "tmpfs";
                    options = [ "size=4G" "mode=0755" ];
                  };
                };

              testScript = ''
                import json

                machine.wait_for_unit("multi-user.target")

                # Make sure both tierfs.service and any previous mounts are stopped and clean
                machine.execute("systemctl stop tierfs.service || true")
                machine.execute("fusermount3 -u /mnt/tierfs/merged || true")
                machine.execute("rm -rf /mnt/tierfs/tier1/* /mnt/tierfs/tier2/* /mnt/tierfs/tier3/*")
                machine.execute("rm -rf /var/lib/tierfs/* /var/lib/tierfs-test/*")

                # Check if tmpfs mounts are mounted
                machine.succeed("mountpoint -q /mnt/tierfs/tier1")
                machine.succeed("mountpoint -q /mnt/tierfs/tier2")
                machine.succeed("mountpoint -q /mnt/tierfs/tier3")

                # ----------------------------------------------------
                # Phase 1: Benchmark Original C++ Autotier
                # ----------------------------------------------------
                print("Starting Phase 1: Autotier (C++)")
                machine.succeed("autotierfs -c /etc/tierfs.conf /mnt/tierfs/merged -o allow_other")
                machine.wait_until_succeeds("mountpoint -q /mnt/tierfs/merged", timeout=10)

                # Write Benchmark (C++)
                cpp_write_out = machine.succeed("fio --name=write --filename=/mnt/tierfs/merged/bench.file --size=50M --rw=write --bs=1M --direct=1 --output-format=json")
                # Read Benchmark (C++)
                cpp_read_out = machine.succeed("fio --name=read --filename=/mnt/tierfs/merged/bench.file --size=50M --rw=read --bs=1M --direct=1 --output-format=json")

                # Cleanup C++ Mount
                machine.succeed("fusermount3 -u /mnt/tierfs/merged")
                machine.execute("rm -rf /mnt/tierfs/tier1/* /mnt/tierfs/tier2/* /mnt/tierfs/tier3/*")
                machine.execute("rm -rf /var/lib/tierfs/* /var/lib/tierfs-test/*")

                # ----------------------------------------------------
                # Phase 2: Benchmark Rust TierFS
                # ----------------------------------------------------
                print("Starting Phase 2: TierFS (Rust)")
                machine.succeed("systemctl start tierfs.service")
                machine.wait_for_unit("tierfs.service")
                machine.wait_until_succeeds("mountpoint -q /mnt/tierfs/merged", timeout=10)

                # Write Benchmark (Rust)
                rust_write_out = machine.succeed("fio --name=write --filename=/mnt/tierfs/merged/bench.file --size=50M --rw=write --bs=1M --direct=1 --output-format=json")
                # Read Benchmark (Rust)
                rust_read_out = machine.succeed("fio --name=read --filename=/mnt/tierfs/merged/bench.file --size=50M --rw=read --bs=1M --direct=1 --output-format=json")

                # Cleanup Rust Mount
                machine.succeed("systemctl stop tierfs.service")

                # ----------------------------------------------------
                # Phase 3: Parsing and Reporting Percentage Comparison
                # ----------------------------------------------------
                cpp_write = json.loads(cpp_write_out)
                cpp_read = json.loads(cpp_read_out)
                rust_write = json.loads(rust_write_out)
                rust_read = json.loads(rust_read_out)

                cpp_write_bw = cpp_write['jobs'][0]['write'].get('bw_bytes', cpp_write['jobs'][0]['write'].get('bw', 0) * 1024)
                cpp_read_bw = cpp_read['jobs'][0]['read'].get('bw_bytes', cpp_read['jobs'][0]['read'].get('bw', 0) * 1024)

                rust_write_bw = rust_write['jobs'][0]['write'].get('bw_bytes', rust_write['jobs'][0]['write'].get('bw', 0) * 1024)
                rust_read_bw = rust_read['jobs'][0]['read'].get('bw_bytes', rust_read['jobs'][0]['read'].get('bw', 0) * 1024)

                write_diff = ((rust_write_bw - cpp_write_bw) / cpp_write_bw) * 100.0 if cpp_write_bw > 0 else 0.0
                read_diff = ((rust_read_bw - cpp_read_bw) / cpp_read_bw) * 100.0 if cpp_read_bw > 0 else 0.0

                print("=" * 60)
                print("                    BENCHMARK RESULTS REPORT")
                print("=" * 60)
                print(f"Sequential Write (C++ Autotier): {cpp_write_bw / (1024*1024):.2f} MB/s")
                print(f"Sequential Write (Rust TierFS):  {rust_write_bw / (1024*1024):.2f} MB/s")
                print(f"Write Performance Difference:     {write_diff:+.2f}% ({'FASTER' if write_diff > 0 else 'SLOWER'})")
                print("-" * 60)
                print(f"Sequential Read (C++ Autotier):  {cpp_read_bw / (1024*1024):.2f} MB/s")
                print(f"Sequential Read (Rust TierFS):   {rust_read_bw / (1024*1024):.2f} MB/s")
                print(f"Read Performance Difference:      {read_diff:+.2f}% ({'FASTER' if read_diff > 0 else 'SLOWER'})")
                print("=" * 60)
              '';
            };
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
