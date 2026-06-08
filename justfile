build:
    cargo build

run:
    cargo run

check:
    cargo clippy
    cargo fmt --check

test-migration: build
    nix develop --command cargo run --bin test_tiering

setup-test:
    #!/usr/bin/env bash
    set -euo pipefail
    
    echo "Cleaning up any old mounts/devices..."
    sudo umount -f test_mounts/merged || true
    sudo umount -f test_mounts/tier1 || true
    sudo umount -f test_mounts/tier2 || true
    sudo umount -f test_mounts/tier3 || true
    
    sudo dmsetup remove tierfs-tier2 || true
    sudo dmsetup remove tierfs-tier3 || true
    
    sudo losetup -D || true
    
    mkdir -p test_images test_mounts/tier1 test_mounts/tier2 test_mounts/tier3 test_mounts/merged run_path
    
    echo "Creating disk images (64M each)..."
    dd if=/dev/zero of=test_images/tier1.img bs=1M count=256 status=none
    dd if=/dev/zero of=test_images/tier2.img bs=1M count=512 status=none
    dd if=/dev/zero of=test_images/tier3.img bs=1M count=1024 status=none
    
    echo "Setting up loop devices..."
    LOOP1=$(sudo losetup -f --show test_images/tier1.img)
    LOOP2=$(sudo losetup -f --show test_images/tier2.img)
    LOOP3=$(sudo losetup -f --show test_images/tier3.img)
    
    echo "Loop devices: $LOOP1, $LOOP2, $LOOP3"
    
    SIZE2=$(sudo blockdev --getsize "$LOOP2")
    SIZE3=$(sudo blockdev --getsize "$LOOP3")
    
    echo "Setting up device delays via dmsetup (Tier 2: 50ms, Tier 3: 200ms)..."
    echo "0 $SIZE2 delay $LOOP2 0 50" | sudo dmsetup create tierfs-tier2
    echo "0 $SIZE3 delay $LOOP3 0 200" | sudo dmsetup create tierfs-tier3
    
    echo "Formatting filesystems..."
    sudo mkfs.ext4 -F "$LOOP1" >/dev/null
    sudo mkfs.ext4 -F /dev/mapper/tierfs-tier2 >/dev/null
    sudo mkfs.ext4 -F /dev/mapper/tierfs-tier3 >/dev/null
    
    echo "Mounting tier filesystems..."
    sudo mount "$LOOP1" test_mounts/tier1
    sudo mount /dev/mapper/tierfs-tier2 test_mounts/tier2
    sudo mount /dev/mapper/tierfs-tier3 test_mounts/tier3
    
    sudo chown -R $(whoami):$(id -gn) test_mounts/tier1 test_mounts/tier2 test_mounts/tier3
    
    echo "Creating local tierfs.conf..."
    echo "[Global]" > test_images/tierfs.conf
    echo "Log Level = 2" >> test_images/tierfs.conf
    echo "Tier Period = 10" >> test_images/tierfs.conf
    echo "Copy Buffer Size = 1 MiB" >> test_images/tierfs.conf
    echo "Run Path = $(pwd)/run_path" >> test_images/tierfs.conf
    echo "" >> test_images/tierfs.conf
    echo "[Tier1]" >> test_images/tierfs.conf
    echo "Path = $(pwd)/test_mounts/tier1" >> test_images/tierfs.conf
    echo "Quota = 80%" >> test_images/tierfs.conf
    echo "" >> test_images/tierfs.conf
    echo "[Tier2]" >> test_images/tierfs.conf
    echo "Path = $(pwd)/test_mounts/tier2" >> test_images/tierfs.conf
    echo "Quota = 80%" >> test_images/tierfs.conf
    echo "" >> test_images/tierfs.conf
    echo "[Tier3]" >> test_images/tierfs.conf
    echo "Path = $(pwd)/test_mounts/tier3" >> test_images/tierfs.conf
    echo "Quota = 80%" >> test_images/tierfs.conf
    
    echo "Test scenario set up successfully!"
    echo "Tiers are mounted in $(pwd)/test_mounts"
    echo "Run 'just mount-fs' to mount the tierfs FUSE filesystem."

cleanup-test:
    #!/usr/bin/env bash
    set -x
    sudo umount -f test_mounts/merged || true
    sudo umount -f test_mounts/tier1 || true
    sudo umount -f test_mounts/tier2 || true
    sudo umount -f test_mounts/tier3 || true
    sudo dmsetup remove tierfs-tier2 || true
    sudo dmsetup remove tierfs-tier3 || true
    sudo losetup -D || true
    rm -rf test_images test_mounts run_path

mount-fs: build
    sudo ./target/debug/tierfs --config test_images/tierfs.conf test_mounts/merged

benchmark:
    nix run .#benchmark

test-nixos:
    nix build .#checks.x86_64-linux.tierfs-test --rebuild -L || nix build .#checks.x86_64-linux.tierfs-test -L
