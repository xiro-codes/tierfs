//! Configuration file parsing for tierfs.conf.

use crate::tier::Tier;
use ini::Ini;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const DEFAULT_CONFIG_PATH: &str = "/etc/tierfs.conf";

/// Log level enum, matching `LogLevel` in the C++ project.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LogLevel {
    None = 0,
    Normal = 1,
    Debug = 2,
}

/// Overrides for config options from CLI flags, matching `ConfigOverrides`.
#[derive(Debug, Clone, Default)]
pub struct ConfigOverrides {
    pub log_level_override: Option<LogLevel>,
}

/// Global configurations parsed from tierfs.conf, matching `Config`.
#[derive(Debug, Clone)]
pub struct Config {
    pub log_level: LogLevel,
    pub copy_buff_sz: usize,
    pub tier_period_s: Duration,
    pub strict_period: bool,
    pub crawler_threads: usize,
    pub run_path: PathBuf,
    pub log_file: Option<PathBuf>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            log_level: LogLevel::Normal,
            copy_buff_sz: 1024 * 1024, // 1 MiB
            tier_period_s: Duration::from_secs(1000),
            strict_period: false,
            crawler_threads: 4,
            run_path: PathBuf::from("/var/lib/tierfs"),
            log_file: None,
        }
    }
}

impl Config {
    /// Load global configurations and tier definitions from the config path.
    pub fn load(
        config_path: &Path,
        overrides: &ConfigOverrides,
    ) -> Result<(Self, Vec<Tier>), String> {
        let mut config = Self::default();
        let mut tiers = Vec::new();

        if !config_path.exists() {
            return Err(format!("Config file does not exist: {:?}", config_path));
        }

        let map = Ini::load_from_file(config_path)
            .map_err(|e| format!("Failed to parse INI config: {}", e))?;

        // Parse global section
        if let Some(global) = map.section(Some("Global")) {
            if let Some(log_val) = global.get("Log Level") {
                config.log_level = match log_val.trim() {
                    "0" => LogLevel::None,
                    "2" => LogLevel::Debug,
                    _ => LogLevel::Normal,
                };
            }
            if let Some(period_val) = global.get("Tier Period")
                && let Ok(secs) = period_val.trim().parse::<u64>()
            {
                config.tier_period_s = Duration::from_secs(secs);
            }
            if let Some(buf_val) = global.get("Copy Buffer Size") {
                config.copy_buff_sz = parse_bytes_string(buf_val).unwrap_or(1024 * 1024);
            }
            if let Some(run_val) = global.get("Run Path") {
                config.run_path = PathBuf::from(run_val.trim());
            }
            if let Some(log_val) = global.get("Log File") {
                config.log_file = Some(PathBuf::from(log_val.trim()));
            }
        }

        // Apply CLI overrides
        if let Some(level) = overrides.log_level_override {
            config.log_level = level;
        }

        // Parse other sections as tiers
        for (sec_name, sec_data) in &map {
            let name = match sec_name {
                Some("Global") | None => continue,
                Some(name) => name,
            };

            if let Some(path_val) = sec_data.get("Path") {
                let path = PathBuf::from(path_val.trim());
                let quota_val = sec_data.get("Quota").map(|s| s.trim()).unwrap_or("80%");

                // Parse quota (e.g. "80%", "5.3 TiB")
                let (quota_bytes, quota_percent) = parse_quota(quota_val);

                tiers.push(Tier::new(
                    name.to_string(),
                    path,
                    quota_bytes,
                    quota_percent,
                ));
            }
        }

        Ok((config, tiers))
    }

    /// Dumps current config to a string stream, matching `dump`.
    pub fn dump(&self, tiers: &[Tier]) -> String {
        let mut out = String::new();
        out.push_str("[Global]\n");
        out.push_str(&format!("Log Level = {:?}\n", self.log_level));
        out.push_str(&format!(
            "Tier Period = {}s\n",
            self.tier_period_s.as_secs()
        ));
        out.push_str(&format!("Copy Buffer Size = {} bytes\n", self.copy_buff_sz));
        out.push_str(&format!("Run Path = {:?}\n", self.run_path));
        if let Some(ref lf) = self.log_file {
            out.push_str(&format!("Log File = {:?}\n", lf));
        }
        out.push('\n');
        for tier in tiers {
            out.push_str(&format!("[{}]\n", tier.id));
            out.push_str(&format!("Path = {:?}\n", tier.path));
            out.push_str(&format!("Quota Bytes = {}\n", tier.quota_bytes));
            out.push_str(&format!("Quota Percent = {}%\n\n", tier.quota_percent));
        }
        out
    }
}

/// Helper to parse quota string to bytes and percentages.
fn parse_quota(val: &str) -> (u64, f64) {
    if val.ends_with('%')
        && let Ok(p) = val[..val.len() - 1].trim().parse::<f64>()
    {
        return (0, p);
    }
    let bytes = parse_bytes_string(val).unwrap_or(0);
    (bytes as u64, 0.0)
}

/// Helper to parse size strings like "1 MiB" or "5.3 TiB".
fn parse_bytes_string(val: &str) -> Option<usize> {
    let trimmed = val.trim();
    let parts: Vec<&str> = trimmed.split_whitespace().collect();
    if parts.is_empty() {
        return None;
    }
    let value = parts[0].parse::<f64>().ok()?;
    if parts.len() < 2 {
        return Some(value as usize);
    }
    let unit = parts[1].to_lowercase();
    let multiplier = match unit.as_str() {
        "kb" | "k" => 1000u64,
        "kib" => 1024u64,
        "mb" | "m" => 1000u64 * 1000,
        "mib" => 1024u64 * 1024,
        "gb" | "g" => 1000u64 * 1000 * 1000,
        "gib" => 1024u64 * 1024 * 1024,
        "tb" | "t" => 1000u64 * 1000 * 1000 * 1000,
        "tib" => 1024u64 * 1024 * 1024 * 1024,
        _ => 1u64,
    };
    Some((value * multiplier as f64) as usize)
}

/// Initializes a default configuration file if it does not exist, matching `init_config_file`.
pub fn init_config_file(config_path: &Path) -> Result<(), String> {
    if config_path.exists() {
        return Ok(());
    }
    if let Some(parent) = config_path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let default_content = r#"# tierfs config
[Global]
Log Level = 1
Tier Period = 1000
Copy Buffer Size = 1 MiB

# [Tier 1]
# Path = /mnt/ssd
# Quota = 80%

# [Tier 2]
# Path = /mnt/hdd
# Quota = 95%
"#;
    fs::write(config_path, default_content)
        .map_err(|e| format!("Failed to write default config: {}", e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_bytes_string() {
        assert_eq!(parse_bytes_string("100"), Some(100));
        assert_eq!(parse_bytes_string("1 kb"), Some(1000));
        assert_eq!(parse_bytes_string("1.5 kib"), Some(1536));
        assert_eq!(parse_bytes_string("2 MB"), Some(2_000_000));
        assert_eq!(parse_bytes_string("1 mib"), Some(1_048_576));
        assert_eq!(parse_bytes_string("10 gib"), Some(10_737_418_240));
    }

    #[test]
    fn test_parse_quota() {
        assert_eq!(parse_quota("80%"), (0, 80.0));
        assert_eq!(parse_quota("50"), (50, 0.0));
        assert_eq!(parse_quota("1 mib"), (1_048_576, 0.0));
    }

    #[test]
    fn test_config_load() {
        let temp_dir = std::env::temp_dir();
        let config_file = temp_dir.join("test_tierfs.conf");
        let content = r#"[Global]
Log Level = 2
Tier Period = 5
Copy Buffer Size = 2 MiB
Run Path = /tmp/tierfs

[FastTier]
Path = /tmp/fast
Quota = 75%

[SlowTier]
Path = /tmp/slow
Quota = 100 MiB
"#;
        fs::write(&config_file, content).unwrap();

        let overrides = ConfigOverrides::default();
        let (config, tiers) = Config::load(&config_file, &overrides).unwrap();

        assert_eq!(config.log_level, LogLevel::Debug);
        assert_eq!(config.tier_period_s, Duration::from_secs(5));
        assert_eq!(config.copy_buff_sz, 2_097_152);
        assert_eq!(config.run_path, PathBuf::from("/tmp/tierfs"));

        assert_eq!(tiers.len(), 2);
        assert_eq!(tiers[0].id, "FastTier");
        assert_eq!(tiers[0].path, PathBuf::from("/tmp/fast"));
        assert_eq!(tiers[0].quota_percent, 75.0);
        assert_eq!(tiers[0].quota_bytes, 0);

        assert_eq!(tiers[1].id, "SlowTier");
        assert_eq!(tiers[1].path, PathBuf::from("/tmp/slow"));
        assert_eq!(tiers[1].quota_percent, 0.0);
        assert_eq!(tiers[1].quota_bytes, 104_857_600);

        let _ = fs::remove_file(&config_file);
    }
}
