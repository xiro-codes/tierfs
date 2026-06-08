//! FUSE daemon entrypoint for tierfs.

use clap::Parser;
use tierfs::config::{ConfigOverrides, LogLevel};
use tierfs::engine::TierEngine;
use tierfs::fuse_fs::TierFS;
use tierfs::tools::fs_usage;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::process;

const VERSION: &str = "0.1.0";

#[derive(Parser, Debug)]
#[command(name = "tierfs", version = VERSION, about = "FUSE daemon for tierfs filesystem")]
struct CliArgs {
    #[arg(short, long, default_value = "/etc/tierfs.conf")]
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
    let log_level = match engine.config.log_level {
        LogLevel::None => "error",
        LogLevel::Normal => "info",
        LogLevel::Debug => "trace",
    };
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(log_level)).init();

    log::info!("Mounting tierfs at {:?}", mountpoint_path);

    let fs = TierFS::new(Arc::clone(&engine), mountpoint_path.clone());

    // Parse FUSE options
    let mut mount_options = vec![
        fuser::MountOption::FSName("tierfs".to_string()),
    ];

    if nix::unistd::Uid::current().is_root() {
        mount_options.push(fuser::MountOption::CUSTOM("allow_other".to_string()));
    }

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
