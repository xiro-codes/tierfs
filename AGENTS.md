# TierFS Agent Guide

This document provides essential context for AI agents working in the TierFS repository.

## Project Overview
TierFS is a Rust port of **Autotier**, a FUSE-based tiered storage filesystem. It dynamically manages multiple underlying storage directories (tiers) of varying speeds and capacities, presenting them as a single mountpoint. Files are transparently migrated between tiers in the background based on file size, access patterns, and tier capacity quotas.

## Architecture & Components
- **FUSE Layer (`src/fuse_fs.rs`)**: Implements the filesystem interface using the `fuser` crate. Delegates requests to physical paths on individual tiers or handles metadata updates.
- **Engine (`src/engine.rs`)**: The core component responsible for managing tiers, background migration, and the overall lifecycle of the filesystem.
- **Metadata (`src/metadata.rs`)**: Interacts with the internal state database (SQLite via `rusqlite`).
- **Tiers (`src/tier.rs`)**: Abstraction representing physical storage locations, tracking usage and quota parameters.

## Configuration
The filesystem is configured via an INI file (commonly `/etc/tierfs.conf` or tested locally). Key sections include:
- `[Global]`: Settings like `Tier Period`, `Copy Buffer Size`, and `Run Path` (where the SQLite DB lives).
- `[TierN]`: Defines individual tiers with their physical `Path` and target `Quota`.

## Development Commands
The project heavily uses `just` as a command runner. Key targets defined in `justfile`:

- `just build` - Compile the project (`cargo build`).
- `just check` - Run lints and formatting checks (`cargo clippy` & `cargo fmt --check`).
- `just setup-test` - Spins up a local test environment by creating loop devices, formatting them as `ext4`, mounting them, and generating a local configuration file.
- `just mount-fs` - Mounts the compiled TierFS FUSE instance based on the generated test configuration.
- `just cleanup-test` - Unmounts and tears down loop devices and test files.
- `just test-nixos` - Runs the NixOS integration tests using `tmpfs` backed tiers.
- `just benchmark` - Runs the performance benchmark comparing Rust TierFS against C++ Autotier.

## Testing & Environment
- **NixOS Integration**: The project uses Nix flakes (`flake.nix`) and includes comprehensive NixOS modules (`nix/nixos-module.nix`) and tests (`nix/nixos-test-module.nix`). The NixOS test spins up a simulated multi-tier environment using `tmpfs`.
- **Local Testing**: The `just setup-test` command uses `dd`, `losetup`, and `dmsetup` to create a realistic test scenario with simulated device delays (e.g., 50ms for Tier 2, 200ms for Tier 3) and mounts them into `test_mounts/`. Agents testing physical FUSE interaction should utilize these `just` targets.

## Important Gotchas & Conventions
- **SQLite Database**: TierFS relies heavily on an internal SQLite database stored at the `Run Path` for maintaining metadata. Agents modifying how files are tracked or moved must ensure they update the SQLite state properly.
- **Background Processes**: FUSE initializes background threads (e.g., for migration) during the `init` phase. If you modify file persistence or metadata, keep concurrent thread access and locking in mind.
- **Path Resolution**: The FUSE layer often needs to resolve virtual paths to their physical equivalents across different tiers. Be careful to check `fuse_fs.rs` `resolve_physical_path` logic when implementing new file-related features.
