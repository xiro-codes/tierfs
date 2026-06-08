import subprocess
import time
import re
import os
import signal

def run_cmd(cmd):
    print(f"Running: {cmd}")
    subprocess.run(cmd, shell=True, check=True)

def main():
    print("Setting up test environment...")
    run_cmd("just setup-test")
    
    # Modify config for faster tiering and debugging
    with open("test_images/tierfs.conf", "r") as f:
        conf = f.read()
    conf = conf.replace("Tier Period = 10", "Tier Period = 1")
    conf = conf.replace("Log Level = 1", "Log Level = 2")
    with open("test_images/tierfs.conf", "w") as f:
        f.write(conf)
        
    print("Mounting tierfs...")
        # Run tierfs and capture output
    tierfs_proc = subprocess.Popen(
        "sudo ./target/debug/tierfs --config test_images/tierfs.conf test_mounts/merged > tierfs.log 2>&1",
        shell=True
    )
    
    time.sleep(2) # Wait for mount
    
    print("Creating files and generating activity...")
    merged = "test_mounts/merged"
    
    # Create files
    run_cmd(f"sudo dd if=/dev/urandom of={merged}/fileA bs=1M count=50 status=none") # 50M
    time.sleep(2)
    run_cmd(f"sudo dd if=/dev/urandom of={merged}/fileB bs=1M count=100 status=none") # 100M
    time.sleep(2)
    try:
        run_cmd(f"sudo dd if=/dev/urandom of={merged}/fileC bs=1M count=100 status=none") # 100M
    except:
        print("fileC hit ENOSPC, expected")
    
    time.sleep(2)
    
    # Make fileA popular
    for _ in range(5):
        try:
            subprocess.run(f"sudo cat {merged}/fileA > /dev/null", shell=True, check=True)
        except subprocess.CalledProcessError:
            print("Failed to cat fileA, tierfs might have crashed.")
        time.sleep(0.5)
        
    # Wait for tiering rounds
    print("Waiting for tiering engine to do its job...")
    time.sleep(10)
    
    print("Unmounting...")
    try:
        run_cmd("sudo umount -f test_mounts/merged")
    except:
        pass
        
    try:
        run_cmd("sudo pkill tierfs")
    except:
        pass
    
    time.sleep(1)
    
    run_cmd("just cleanup-test")
    
    with open("tierfs.log", "r") as f:
        stdout = f.read()
    
    # Parse logs
    print("\n--- MIGRATION VISUALIZATION ---")
    print(f"{'TIME':<25} | {'EVENT':<22} | {'FILE':<10} | {'FROM -> TO':<15} | {'DETAILS'}")
    print("-" * 100)
    
    for line in stdout.splitlines():
        if "MIGRATION_" in line:
            # Example: [2026-06-08T12:06:41.187Z INFO tierfs] MIGRATION_DECISION: file="fileA" old_tier=tier2 new_tier=tier1 size=10485760 popularity=1.5
            match = re.search(r'\[(.*?) (.*?)\] MIGRATION_(DECISION|SUCCESS|FAILURE|SUCCESS_COPY): (.*)', line)
            if match:
                timestamp = match.group(1)
                event = match.group(3)
                details = match.group(4)
                
                file_match = re.search(r'file="?([^"\s]+)"?', details)
                filename = file_match.group(1) if file_match else "?"
                
                from_tier = "?"
                to_tier = "?"
                
                if event == "DECISION":
                    o_match = re.search(r'old_tier=([^ ]+)', details)
                    n_match = re.search(r'new_tier=([^ ]+)', details)
                    if o_match: from_tier = o_match.group(1)
                    if n_match: to_tier = n_match.group(1)
                elif "SUCCESS" in event:
                    o_match = re.search(r'old_path="?[^"]*tier([0-9]+)', details)
                    n_match = re.search(r'new_path="?[^"]*tier([0-9]+)', details)
                    if o_match: from_tier = "Tier" + o_match.group(1)
                    if n_match: to_tier = "Tier" + n_match.group(1)
                    
                path_str = f"{from_tier} -> {to_tier}"
                if event == "DECISION":
                    path_str = f"{from_tier} -> {to_tier}"
                
                print(f"{timestamp:<25} | {event:<22} | {filename:<10} | {path_str:<15} | {details}")

if __name__ == "__main__":
    main()
