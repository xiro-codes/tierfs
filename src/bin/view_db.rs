//! Utility to view the contents of the tierfs metadata database.

use clap::Parser;
use rusqlite::Connection;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process;
use tierfs::config::{Config, ConfigOverrides};

const VERSION: &str = "0.1.0";

#[derive(Parser, Debug)]
#[command(name = "view_db", version = VERSION, about = "View the tierfs metadata SQLite database")]
struct CliArgs {
    #[arg(short, long, default_value = "/etc/tierfs.conf")]
    config: String,
}

fn get_run_path(config_path: &Path) -> PathBuf {
    let overrides = ConfigOverrides::default();
    let base_run_path = match Config::load(config_path, &overrides) {
        Ok((cfg, _)) => cfg.run_path,
        Err(_) => PathBuf::from("/var/lib/tierfs"),
    };

    let mut hasher = DefaultHasher::new();
    config_path.to_string_lossy().to_string().hash(&mut hasher);
    let hash_val = hasher.finish();

    base_run_path.join(hash_val.to_string())
}

struct Row {
    key: String,
    tier_path: String,
    access_count: i64,
    popularity: f64,
    pinned: bool,
}

fn main() {
    let args = CliArgs::parse();
    let config_path = Path::new(&args.config);

    // Attempt to load run path normally first, fallback to hashed path for daemon compatibility
    let overrides = ConfigOverrides::default();
    let run_path = match Config::load(config_path, &overrides) {
        Ok((cfg, _)) => cfg.run_path,
        Err(_) => get_run_path(config_path),
    };

    let db_path = run_path.join("metadata.db");

    if !db_path.exists() {
        eprintln!("Error: Database not found at {:?}", db_path);
        process::exit(1);
    }

    let conn = match Connection::open(&db_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error: Failed to open SQLite DB: {}", e);
            process::exit(1);
        }
    };

    let mut stmt = match conn
        .prepare("SELECT relative_path, tier_path, access_count, popularity, pinned FROM metadata")
    {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Error: Failed to prepare query: {}", e);
            process::exit(1);
        }
    };

    let row_iter = stmt
        .query_map([], |row| {
            Ok(Row {
                key: row.get(0)?,
                tier_path: row.get(1)?,
                access_count: row.get(2)?,
                popularity: row.get(3)?,
                pinned: {
                    let p: i32 = row.get(4)?;
                    p != 0
                },
            })
        })
        .unwrap();

    let mut rows = Vec::new();
    for r in row_iter.flatten() {
        rows.push(r);
    }

    if rows.is_empty() {
        println!("Database is empty.");
        return;
    }

    let key_header = "Key";
    let tier_header = "Tier";
    let acount_header = "Access Count";
    let pop_header = "Popularity";
    let pinned_header = "Pinned";

    let mut key_len = key_header.len();
    let mut tpath_len = tier_header.len();
    let mut acnt_len = acount_header.len();
    let mut pop_len = pop_header.len();
    let pinned_len = 6; // length of "Pinned" or "false"

    for r in &rows {
        key_len = key_len.max(r.key.len());
        tpath_len = tpath_len.max(r.tier_path.len());
        acnt_len = acnt_len.max(r.access_count.to_string().len());
        pop_len = pop_len.max(format!("{:.4}", r.popularity).len());
    }

    // Print headers
    println!(
        "{:<k_width$} | {:<t_width$}  {:<a_width$}  {:<p_width$}  {:<pin_width$}",
        key_header,
        tier_header,
        acount_header,
        pop_header,
        pinned_header,
        k_width = key_len,
        t_width = tpath_len,
        a_width = acnt_len,
        p_width = pop_len,
        pin_width = pinned_len
    );

    // Print separator
    let total_len = key_len + 3 + tpath_len + 2 + acnt_len + 2 + pop_len + 2 + pinned_len;
    let mut sep = "-".repeat(key_len + 1);
    sep.push('+');
    sep.push_str(&"-".repeat(total_len - sep.len()));
    println!("{}", sep);

    // Print rows
    for r in &rows {
        println!(
            "{:<k_width$} | {:<t_width$}  {:<a_width$}  {:<p_width$.4}  {:<pin_width$}",
            r.key,
            r.tier_path,
            r.access_count,
            r.popularity,
            r.pinned,
            k_width = key_len,
            t_width = tpath_len,
            a_width = acnt_len,
            p_width = pop_len,
            pin_width = pinned_len
        );
    }
}
