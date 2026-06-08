use tierfs::config::ConfigOverrides;
use tierfs::engine::TierEngine;
use tierfs::metadata::Metadata;
use std::fs;
use std::path::PathBuf;

fn main() -> Result<(), String> {
    println!("=== TierFS Integration Test ===");

    // 1. Set up temporary test directory structure
    let test_dir = PathBuf::from("target/test_tiering_env");
    if test_dir.exists() {
        let _ = fs::remove_dir_all(&test_dir);
    }
    fs::create_dir_all(&test_dir).map_err(|e| e.to_string())?;

    let tier1_dir = test_dir.join("tier1_ssd");
    let tier2_dir = test_dir.join("tier2_sas");
    let tier3_dir = test_dir.join("tier3_sata");

    fs::create_dir_all(&tier1_dir).map_err(|e| e.to_string())?;
    fs::create_dir_all(&tier2_dir).map_err(|e| e.to_string())?;
    fs::create_dir_all(&tier3_dir).map_err(|e| e.to_string())?;

    println!("Created tier directories:");
    println!("  Tier 1 (SSD):   {:?}", tier1_dir);
    println!("  Tier 2 (SAS):   {:?}", tier2_dir);
    println!("  Tier 3 (SATA):  {:?}", tier3_dir);

    // 2. Create the configuration file
    let config_path = test_dir.join("tierfs.conf");
    let config_content = format!(
        r#"[Global]
Log Level = 2
Tier Period = 1
Copy Buffer Size = 4 KiB
Run Path = {}/run_path

[Tier1]
Path = {}
Quota = 50

[Tier2]
Path = {}
Quota = 50

[Tier3]
Path = {}
Quota = 100 MiB
"#,
        test_dir.to_string_lossy(),
        tier1_dir.to_string_lossy(),
        tier2_dir.to_string_lossy(),
        tier3_dir.to_string_lossy()
    );
    fs::write(&config_path, config_content).map_err(|e| e.to_string())?;

    // 3. Create three test files, placing them initially in the slowest tier (Tier 3)
    let hot_file_rel = "hot_file.txt";
    let warm_file_rel = "warm_file.txt";
    let cold_file_rel = "cold_file.txt";

    let hot_path = tier3_dir.join(hot_file_rel);
    let warm_path = tier3_dir.join(warm_file_rel);
    let cold_path = tier3_dir.join(cold_file_rel);

    fs::write(&hot_path, "HOT FILE CONTENT - should migrate to Tier 1").map_err(|e| e.to_string())?;
    fs::write(&warm_path, "WARM FILE CONTENT - should migrate to Tier 2").map_err(|e| e.to_string())?;
    fs::write(&cold_path, "COLD FILE CONTENT - should stay in Tier 3").map_err(|e| e.to_string())?;

    println!("Initialized files in Tier 3 (SATA):");
    println!("  - {:?}", hot_path);
    println!("  - {:?}", warm_path);
    println!("  - {:?}", cold_path);

    // 4. Initialize TierEngine
    let overrides = ConfigOverrides::default();
    let engine = TierEngine::new(&config_path, &overrides)?;

    // 5. Pre-populate SQLite database with high access count for hot/warm files
    {
        let db = engine.db.lock().unwrap();
        // Insert hot file metadata (High popularity/accesses)
        let mut hot_meta = Metadata::default();
        hot_meta.access_count = 100;
        hot_meta.popularity = 2000.0;
        hot_meta.tier_path = tier3_dir.to_string_lossy().into_owned();
        hot_meta.update(&db, hot_file_rel, None).map_err(|e| e.to_string())?;

        // Insert warm file metadata (Medium popularity/accesses)
        let mut warm_meta = Metadata::default();
        warm_meta.access_count = 20;
        warm_meta.popularity = 1000.0;
        warm_meta.tier_path = tier3_dir.to_string_lossy().into_owned();
        warm_meta.update(&db, warm_file_rel, None).map_err(|e| e.to_string())?;

        // Cold file metadata (0 accesses, low popularity)
        let mut cold_meta = Metadata::default();
        cold_meta.access_count = 0;
        cold_meta.popularity = 0.0;
        cold_meta.tier_path = tier3_dir.to_string_lossy().into_owned();
        cold_meta.update(&db, cold_file_rel, None).map_err(|e| e.to_string())?;
    }
    println!("Populated access frequency stats in SQLite database.");

    // 6. Run the tiering migration iteration
    println!("Running engine.tier() to migrate files...");
    let migrated = engine.tier();
    println!("Tiering iteration completed: {}", migrated);

    // 7. Verify the final physical file paths
    let hot_in_tier1 = tier1_dir.join(hot_file_rel).exists();
    let hot_in_tier3 = tier3_dir.join(hot_file_rel).exists();

    let warm_in_tier2 = tier2_dir.join(warm_file_rel).exists();
    let warm_in_tier3 = tier3_dir.join(warm_file_rel).exists();

    let cold_in_tier3 = tier3_dir.join(cold_file_rel).exists();

    println!("\n=== File Location Verification ===");
    println!(
        "hot_file.txt in Tier 1: {} (expected: true)",
        if hot_in_tier1 { "PASS" } else { "FAIL" }
    );
    println!(
        "hot_file.txt removed from Tier 3: {} (expected: true)",
        if !hot_in_tier3 { "PASS" } else { "FAIL" }
    );

    println!(
        "warm_file.txt in Tier 2: {} (expected: true)",
        if warm_in_tier2 { "PASS" } else { "FAIL" }
    );
    println!(
        "warm_file.txt removed from Tier 3: {} (expected: true)",
        if !warm_in_tier3 { "PASS" } else { "FAIL" }
    );

    println!(
        "cold_file.txt remained in Tier 3: {} (expected: true)",
        if cold_in_tier3 { "PASS" } else { "FAIL" }
    );

    // 8. Print database state
    {
        let db = engine.db.lock().unwrap();
        println!("\n=== Final SQLite Database State ===");
        let mut stmt = db
            .prepare("SELECT relative_path, access_count, popularity, tier_path FROM metadata")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |row| {
                let path: String = row.get(0)?;
                let count: i64 = row.get(1)?;
                let pop: f64 = row.get(2)?;
                let tier: String = row.get(3)?;
                Ok((path, count, pop, tier))
            })
            .map_err(|e| e.to_string())?;

        for row in rows.flatten() {
            println!(
                "  File: {:<15} | Accesses (Reset): {:<3} | Popularity: {:<6.2} | Tier Path: {}",
                row.0, row.1, row.2, row.3
            );
        }
    }

    // Clean up
    fs::remove_dir_all(&test_dir).map_err(|e| e.to_string())?;
    println!("\nCleaned up test environment.");

    if hot_in_tier1 && !hot_in_tier3 && warm_in_tier2 && !warm_in_tier3 && cold_in_tier3 {
        println!("INTEGRATION TEST SUCCESSFUL!");
        Ok(())
    } else {
        Err("INTEGRATION TEST FAILED!".to_string())
    }
}
