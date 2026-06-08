//! CLI client for autotier.

use clap::Parser;
use rust_cli::config::{Config, ConfigOverrides, LogLevel};
use rust_cli::tools::{cli_usage, get_command_index, Command};

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process;

const VERSION: &str = "0.1.0";

#[derive(Parser, Debug)]
#[command(name = "autotier", version = VERSION, about = "CLI client for autotier filesystem")]
struct CliArgs {
    #[arg(short, long, default_value = "/etc/autotier.conf")]
    config: String,

    #[arg(short, long)]
    json: bool,

    #[arg(short, long)]
    verbose: bool,

    #[arg(short, long)]
    quiet: bool,

    // Command and arguments are parsed manually to align with the C++ client structure
    #[arg(trailing_var_arg = true)]
    command_and_args: Vec<String>,
}

fn get_run_path(config_path: &Path) -> PathBuf {
    // Attempt to parse global run path from the INI file
    let overrides = ConfigOverrides::default();
    let base_run_path = match Config::load(config_path, &overrides) {
        Ok((cfg, _)) => cfg.run_path,
        Err(_) => PathBuf::from("/var/lib/autotier"),
    };

    // Match the C++ hash subfolder logic: std::hash<std::string>{}(config_path)
    let mut hasher = DefaultHasher::new();
    config_path.to_string_lossy().to_string().hash(&mut hasher);
    let hash_val = hasher.finish();

    base_run_path.join(hash_val.to_string())
}

fn main() {
    let args = CliArgs::parse();

    if args.command_and_args.is_empty() {
        eprintln!("Error: No command passed.");
        cli_usage();
        process::exit(1);
    }

    let cmd_str = &args.command_and_args[0];
    let cmd = match get_command_index(cmd_str) {
        Some(c) => c,
        None => {
            eprintln!("Error: Invalid command: {}", cmd_str);
            cli_usage();
            process::exit(1);
        }
    };

    if cmd == Command::Help {
        cli_usage();
        process::exit(0);
    }

    // Prepare socket payload
    let mut payload = Vec::new();
    payload.push(cmd_str.clone());

    let mut arg_idx = 1;
    if cmd == Command::Pin {
        if args.command_and_args.len() < 2 {
            eprintln!("Error: No arguments passed (missing target tier).");
            process::exit(1);
        }
        payload.push(args.command_and_args[1].clone()); // Push tier name
        arg_idx = 2;
    }

    // Push paths or other arguments
    if cmd == Command::Pin || cmd == Command::Unpin || cmd == Command::WhichTier {
        if args.command_and_args.len() <= arg_idx {
            eprintln!("Error: No arguments passed.");
            process::exit(1);
        }
        for path_arg in &args.command_and_args[arg_idx..] {
            // Normalize path to absolute
            let abs_path = std::fs::canonicalize(path_arg)
                .unwrap_or_else(|_| PathBuf::from(path_arg));
            payload.push(abs_path.to_string_lossy().to_string());
        }
    } else if cmd == Command::Status {
        payload.push(args.json.to_string());
    }

    let config_path = Path::new(&args.config);
    let run_path = get_run_path(config_path);
    let socket_path = run_path.join("adhoc.socket");

    // Connect and send payload
    let mut stream = match UnixStream::connect(&socket_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Socket connection refused. Is autotierfs mounted? ({})", e);
            process::exit(1);
        }
    };

    let serialized = serde_json::to_string(&payload).unwrap_or_default();
    if let Err(e) = stream.write_all(serialized.as_bytes()) {
        eprintln!("Socket write error: {}", e);
        process::exit(1);
    }

    let mut response_buf = Vec::new();
    if let Err(e) = stream.read_to_end(&mut response_buf) {
        eprintln!("Socket read error: {}", e);
        process::exit(1);
    }

    let response: Vec<String> = match serde_json::from_slice(&response_buf) {
        Ok(r) => r,
        Err(_) => {
            eprintln!("Error: Failed to parse socket response.");
            process::exit(1);
        }
    };

    if response.is_empty() {
        eprintln!("Error: Empty response from daemon.");
        process::exit(1);
    }

    if response[0] == "OK" {
        for line in &response[1..] {
            println!("{}", line);
        }
    } else {
        for line in &response[1..] {
            eprintln!("{}", line);
        }
        process::exit(1);
    }
}
