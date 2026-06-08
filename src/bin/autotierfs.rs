//! FUSE daemon entrypoint for autotier.

use clap::Parser;
use rust_cli::config::{ConfigOverrides, LogLevel};
use rust_cli::engine::TierEngine;
use rust_cli::fuse_fs::AutotierFS;
use rust_cli::tools::fs_usage;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::process;

const VERSION: &str = "0.1.0";

#[derive(Parser, Debug)]
#[command(name = "autotierfs", version = VERSION, about = "FUSE daemon for autotier filesystem")]
struct CliArgs {
    #[arg(short, long, default_value = "/etc/autotier.conf")]
    config: String,

    #[arg(short, long)]
    fuse_options: Option<String>,

    #[arg(short, long)]
    verbose: bool,

    #[arg(short, long)]
    quiet: bool,

    #[arg(required = true)]
    mountpoint: String,
}

fn main() {
    let args = CliArgs::parse();

    let mountpoint_path = PathBuf::from(&args.mountpoint);
    if !mountpoint_path.is_dir() {
        eprintln!("Error: Invalid mountpoint: {}", args.mountpoint);
        fs_usage();
        process::exit(1);
    }

    // Set config overrides
    let mut overrides = ConfigOverrides::default();
    if args.verbose {
        overrides.log_level_override = Some(LogLevel::Debug);
    } else if args.quiet {
        overrides.log_level_override = Some(LogLevel::None);
    }

    let config_path = Path::new(&args.config);
    let engine = match TierEngine::new(config_path, &overrides) {
        Ok(eng) => Arc::new(eng),
        Err(e) => {
            eprintln!("Error: {}", e);
            process::exit(1);
        }
    };

    engine.set_mount_point(mountpoint_path.clone());

    // Setup logging (initialize env_logger)
    env_logger::init();

    log::info!("Mounting autotierfs at {:?}", mountpoint_path);

    let fs = AutotierFS::new(Arc::clone(&engine), mountpoint_path.clone());

    // Parse FUSE options
    let mut mount_options = vec![
        fuser::MountOption::FSName("autotierfs".to_string()),
        fuser::MountOption::CUSTOM("allow_other".to_string()),
    ];

    if let Some(opts) = args.fuse_options {
        for opt in opts.split(',') {
            let opt = opt.trim();
            if !opt.is_empty() {
                mount_options.push(fuser::MountOption::CUSTOM(opt.to_string()));
            }
        }
    }

    let mut config = fuser::Config::default();
    config.mount_options = mount_options;

    if let Err(e) = fuser::mount2(fs, &mountpoint_path, &config) {
        eprintln!("Error mounting filesystem: {}", e);
        process::exit(1);
    }
}
