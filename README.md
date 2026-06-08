# TierFS

TierFS is a Rust port of **Autotier**, a FUSE-based tiered storage filesystem. It merges multiple underlying directories (tiers) of varying speeds and capacities (e.g., SSD, HDD, slow archive disk) into a single mountpoint. Files are dynamically migrated between tiers in the background based on file size, access patterns, and tier capacity quotas.

---

## Configuration

TierFS uses an INI configuration file (typically `/etc/tierfs.conf`):

```ini
[Global]
Log Level = 1
Tier Period = 1000          # Background migration interval (seconds)
Copy Buffer Size = 1 MiB    # Buffer size for migrating files
Run Path = /var/lib/tierfs  # Internal state database path

[Tier1]
Path = /mnt/fast-ssd
Quota = 80%                 # Target utilization threshold

[Tier2]
Path = /mnt/slower-hdd
Quota = 90%
```

---

## How to Run

### Command Line
Mount the filesystem manually:
```bash
tierfs --config /etc/tierfs.conf /mnt/merged
```

### NixOS Configuration
To integrate the daemon into your NixOS system, add `tierfs` as a flake input and import the module:

```nix
# flake.nix
{
  inputs.tierfs.url = "git+file:///path/to/tierfs"; # Or GitHub repository URL
  
  outputs = { self, nixpkgs, tierfs, ... }: {
    nixosConfigurations.myhost = nixpkgs.lib.nixosSystem {
      system = "x86_64-linux";
      modules = [
        tierfs.nixosModules.default
        ({ pkgs, ... }: {
          services.tierfs = {
            enable = true;
            mountPoint = "/mnt/merged";
            tiers = {
              Tier1 = { path = "/mnt/ssd"; quota = "80%"; };
              Tier2 = { path = "/mnt/hdd"; quota = "90%"; };
            };
          };
        })
      ];
    };
  };
}
```

### NixOS Test Environment (Tiers via `tmpfs`)
Import `tierfs.nixosModules.test` instead to spin up a pre-configured test environment using 1G, 2G, and 4G `tmpfs` mounts:
```nix
services.tierfs-test.enable = true;
```

---

## Testing & Benchmarks

To run the integration test and execute a side-by-side benchmark comparing Rust TierFS against C++ Autotier:
```bash
just test-nixos
```

---

## Credits & Attribution

TierFS is a modern Rust rewrite of **autotier**, originally authored by **Joshua Boudreau** (<jboudreau@45drives.com>) / **45Drives**. The Rust port retains configuration compatibility and functional behavior while updating the storage engine backend.
