//! Helper utilities, CLI command enums, and ad-hoc task representation.

use std::path::PathBuf;

/// CLI / socket command definitions, matching `command_enum` in the C++ project.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    OneShot = 0,
    Pin = 1,
    Unpin = 2,
    Status = 3,
    Config = 4,
    Help = 5,
    LPin = 6,
    LPop = 7,
    WhichTier = 8,
}

/// Helper to parse a command string into a Command enum, matching `get_command_index`.
pub fn get_command_index(cmd: &str) -> Option<Command> {
    match cmd.to_lowercase().as_str() {
        "oneshot" | "tier" => Some(Command::OneShot),
        "pin" => Some(Command::Pin),
        "unpin" => Some(Command::Unpin),
        "status" => Some(Command::Status),
        "config" => Some(Command::Config),
        "help" => Some(Command::Help),
        "lpin" | "list-pins" => Some(Command::LPin),
        "lpop" | "list-popularity" => Some(Command::LPop),
        "whichtier" | "which-tier" => Some(Command::WhichTier),
        _ => None,
    }
}

/// Representation of an ad hoc command with the command enum and arguments, matching `AdHoc`.
#[derive(Debug, Clone)]
pub struct AdHoc {
    pub cmd: Command,
    pub args: Vec<String>,
}

impl AdHoc {
    /// Construct a new AdHoc job from Command and arguments.
    pub fn new(cmd: Command, args: Vec<String>) -> Self {
        Self { cmd, args }
    }

    /// Construct a new AdHoc job from a list of payload strings, where the first element is the command.
    pub fn from_payload(payload: &[String]) -> Option<Self> {
        if payload.is_empty() {
            return None;
        }
        let cmd = get_command_index(&payload[0])?;
        let args = payload[1..].to_vec();
        Some(Self { cmd, args })
    }
}

/// Sanitizes paths by verifying they are in the tierfs filesystem, matching `sanitize_paths`.
pub fn sanitize_paths(paths: &mut Vec<PathBuf>) {
    // Stub: In the real implementation, we would check if they are within our mount point.
    // For the stub, we just filter out empty paths.
    paths.retain(|p| !p.as_os_str().is_empty());
}

/// Print usage help message for tierfs to stdout.
pub fn fs_usage() {
    println!("Usage: tierfs [options] <mountpoint>");
    println!("Options:");
    println!("  -c, --config <path>      Path to config file (default: /etc/tierfs.conf)");
    println!("  -o, --fuse-options <opt> Comma separated options to pass to FUSE");
    println!("  -v, --verbose            Enable verbose logging");
    println!("  -q, --quiet              Disable logging");
    println!("  -V, --version            Print version and exit");
    println!("  -h, --help               Print this help message");
}

/// Print usage help message for tierfs client to stdout.
pub fn cli_usage() {
    println!("Usage: tierfs [options] <command> [args]");
    println!("Options:");
    println!("  -c, --config <path>      Path to config file (default: /etc/tierfs.conf)");
    println!("  -j, --json               Output as JSON (for status command)");
    println!("  -v, --verbose            Enable verbose logging");
    println!("  -q, --quiet              Disable logging");
    println!("  -V, --version            Print version and exit");
    println!("  -h, --help               Print this help message");
    println!("\nCommands:");
    println!("  tier                     Force a tiering cycle immediately");
    println!("  status                   Show the current status of each tier");
    println!("  pin <tier> <files...>    Pin files to a specific tier");
    println!("  unpin <files...>         Unpin files to make them eligible for tiering");
    println!("  list-pins                List all pinned files and their tiers");
    println!("  list-popularity          List all tracked files and their popularity score");
    println!("  which-tier <files...>    Determine which tier holds the specified files");
}

/// Helper to update SQLite database entries for all files within a renamed directory.
pub fn update_keys_in_directory(
    conn: &rusqlite::Connection,
    old_directory: &str,
    new_directory: &str,
) -> Result<(), rusqlite::Error> {
    let mut old_prefix = old_directory.trim_start_matches('/').to_string();
    let mut new_prefix = new_directory.trim_start_matches('/').to_string();

    if !old_prefix.ends_with('/') {
        old_prefix.push('/');
    }
    if !new_prefix.ends_with('/') {
        new_prefix.push('/');
    }

    let query = "UPDATE metadata
                 SET relative_path = ?1 || SUBSTR(relative_path, LENGTH(?2) + 1)
                 WHERE relative_path LIKE ?3";

    let like_pattern = format!("{}%", old_prefix);
    conn.execute(
        query,
        rusqlite::params![new_prefix, old_prefix, like_pattern],
    )?;
    Ok(())
}
