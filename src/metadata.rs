//! File metadata stored in the SQLite database.

use serde::{Deserialize, Serialize};
use rusqlite::{params, Connection, OptionalExtension};
use crate::popularity::{AVG_USAGE, MULTIPLIER};

/// Metadata stored in and retrieved from the SQLite database.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Metadata {
    /// Number of times the file was accessed since last tiering cycle.
    pub access_count: u64,
    /// Moving average of file usage frequency in accesses per hour.
    pub popularity: f64,
    /// Flag set to true if metadata was not found in the database.
    #[serde(skip)]
    pub not_found: bool,
    /// Flag to determine whether to ignore file while tiering (keep on current tier).
    pub pinned: bool,
    /// The backend path of the tier containing this file.
    pub tier_path: String,
}

impl Default for Metadata {
    fn default() -> Self {
        Self {
            access_count: 0,
            popularity: MULTIPLIER * AVG_USAGE,
            not_found: false,
            pinned: false,
            tier_path: String::new(),
        }
    }
}

impl Metadata {
    /// Construct a new empty Metadata object.
    pub fn new() -> Self {
        Self::default()
    }

    /// Construct a Metadata object from a serialized JSON string.
    pub fn from_serialized(serialized: &str) -> Result<Self, serde_json::Error> {
        let mut meta: Self = serde_json::from_str(serialized)?;
        meta.not_found = false;
        Ok(meta)
    }

    /// Retrieve metadata from the SQLite database for a given path.
    pub fn from_db(conn: &Connection, relative_path: &str, tier_path: Option<&str>) -> Self {
        let query = "SELECT access_count, popularity, pinned, tier_path FROM metadata WHERE relative_path = ?1";
        let res = conn.query_row(query, params![relative_path], |row| {
            let access_count: i64 = row.get(0)?;
            let popularity: f64 = row.get(1)?;
            let pinned_int: i32 = row.get(2)?;
            let tier_path: String = row.get(3)?;
            Ok(Self {
                access_count: access_count as u64,
                popularity,
                not_found: false,
                pinned: pinned_int != 0,
                tier_path,
            })
        });

        match res.optional() {
            Ok(Some(meta)) => meta,
            _ => {
                let mut m = Self::default();
                if let Some(tp) = tier_path {
                    m.tier_path = tp.to_string();
                }
                m.not_found = true;
                m
            }
        }
    }

    /// Put metadata into the database.
    pub fn update(&self, conn: &Connection, relative_path: &str, old_key: Option<&str>) -> Result<(), rusqlite::Error> {
        if let Some(ok) = old_key {
            if ok != relative_path {
                let delete_query = "DELETE FROM metadata WHERE relative_path = ?1";
                let _ = conn.execute(delete_query, params![ok]);
            }
        }

        let query = "INSERT INTO metadata (relative_path, access_count, popularity, pinned, tier_path)
                     VALUES (?1, ?2, ?3, ?4, ?5)
                     ON CONFLICT(relative_path) DO UPDATE SET
                     access_count = excluded.access_count,
                     popularity = excluded.popularity,
                     pinned = excluded.pinned,
                     tier_path = excluded.tier_path";
        conn.execute(
            query,
            params![
                relative_path,
                self.access_count as i64,
                self.popularity,
                if self.pinned { 1 } else { 0 },
                self.tier_path
            ],
        )?;
        Ok(())
    }

    /// Increments access count, matching `touch`.
    pub fn touch(&mut self) {
        self.access_count += 1;
    }

    /// Return metadata as a formatted string, matching `dump_stats`.
    pub fn dump_stats(&self) -> String {
        format!(
            "Tier Path: {}\nAccesses since last run: {}\nPopularity: {:.4} accesses/hr\nPinned: {}\n",
            self.tier_path, self.access_count, self.popularity, self.pinned
        )
    }
}
