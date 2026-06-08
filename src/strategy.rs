use crate::engine::TierEngine;
use crate::metadata::Metadata;
use crate::popularity::{WEEK, calculate_popularity};
use crate::tier::{File, Tier};
use std::fs;
use std::path::Path;
use std::sync::atomic::Ordering;
use std::thread;
use std::time::{Duration, SystemTime};

pub trait TieringStrategy: Send + Sync {
    fn name(&self) -> &str;
    fn gather_files(&self, engine: &TierEngine, tiers: &mut [Tier]) -> Vec<File>;
    fn calc_popularity(&self, engine: &TierEngine, files: &mut [File]);
    fn sort(&self, engine: &TierEngine, files: &mut [File]);
    fn simulate_tier(&self, engine: &TierEngine, files: &mut [File], tiers: &mut [Tier]);
    fn move_files(&self, engine: &TierEngine, tiers: &mut [Tier]);

    fn execute(&self, engine: &TierEngine) -> bool {
        if engine.currently_tiering.swap(true, Ordering::SeqCst) {
            return false;
        }

        log::debug!("Gathering files.");
        let mut tiers = match engine.tiers.lock() {
            Ok(t) => t,
            Err(_) => {
                engine.currently_tiering.store(false, Ordering::SeqCst);
                return false;
            }
        };

        let mut candidate_files = self.gather_files(engine, &mut tiers);
        self.calc_popularity(engine, &mut candidate_files);
        self.sort(engine, &mut candidate_files);
        self.simulate_tier(engine, &mut candidate_files, &mut tiers);
        self.move_files(engine, &mut tiers);

        // Update database
        if let Ok(db_conn) = engine.db.lock() {
            for file in &candidate_files {
                let _ = file
                    .metadata
                    .update(&db_conn, &file.relative_path.to_string_lossy(), None);
            }
        }

        engine.currently_tiering.store(false, Ordering::SeqCst);
        true
    }
}

pub struct PopularityTieringStrategy;

impl PopularityTieringStrategy {
    pub fn crawl(
        engine: &TierEngine,
        dir: &Path,
        tier: &Tier,
        files: &mut Vec<File>,
        usage: &mut u64,
    ) {
        log::trace!("crawl: dir={:?}, tier={}", dir, tier.id);
        if !dir.is_dir() {
            return;
        }
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                let metadata = match entry.metadata() {
                    Ok(m) => m,
                    Err(_) => continue,
                };

                if metadata.is_dir() {
                    Self::crawl(engine, &path, tier, files, usage);
                } else if !metadata.is_symlink() {
                    let filename = path.file_name().unwrap_or_default().to_string_lossy();
                    if filename.starts_with('.') && filename.ends_with(".tierfs.hide") {
                        continue;
                    }

                    // Get relative path to tier
                    let rel_path = path.strip_prefix(&tier.path).unwrap_or(&path).to_path_buf();
                    let file_size = metadata.len();
                    *usage += file_size;

                    // Retrieve or create DB metadata
                    let db_meta = if let Ok(db_conn) = engine.db.lock() {
                        Metadata::from_db(
                            &db_conn,
                            &rel_path.to_string_lossy(),
                            Some(&tier.path.to_string_lossy()),
                        )
                    } else {
                        Metadata::default()
                    };

                    if !db_meta.pinned {
                        files.push(File::new(rel_path, tier.id.clone(), file_size, db_meta));
                    }
                }
            }
        }
    }
}

impl TieringStrategy for PopularityTieringStrategy {
    fn name(&self) -> &str {
        "popularity"
    }

    fn gather_files(&self, engine: &TierEngine, tiers: &mut [Tier]) -> Vec<File> {
        let mut candidate_files = Vec::new();
        for tier in tiers.iter_mut() {
            let mut usage = 0;
            Self::crawl(engine, &tier.path, tier, &mut candidate_files, &mut usage);
            if let Ok(mut u) = tier.usage_bytes.lock() {
                *u = usage;
            }
        }
        candidate_files
    }

    fn calc_popularity(&self, engine: &TierEngine, files: &mut [File]) {
        log::trace!("calc_popularity for {} files", files.len());
        let mut last_time = engine.last_tier_time.lock().unwrap();
        let now = SystemTime::now();
        let elapsed = now
            .duration_since(*last_time)
            .unwrap_or(Duration::from_secs(1));
        *last_time = now;

        let period_secs = elapsed.as_secs_f64();
        for file in files.iter_mut() {
            // Touch access counts or calc EMA
            let current_pop = file.metadata.popularity;
            let accesses = file.metadata.access_count;
            file.metadata.access_count = 0; // reset
            file.metadata.popularity =
                calculate_popularity(current_pop, accesses, period_secs, WEEK); // mock week age
        }
    }

    fn sort(&self, _engine: &TierEngine, files: &mut [File]) {
        log::trace!("sort: sorting {} files", files.len());
        files.sort_by(|a, b| {
            let pop_a = a.metadata.popularity;
            let pop_b = b.metadata.popularity;
            if (pop_a - pop_b).abs() < 1e-9 {
                b.atime.cmp(&a.atime)
            } else {
                pop_b
                    .partial_cmp(&pop_a)
                    .unwrap_or(std::cmp::Ordering::Equal)
            }
        });
    }

    fn simulate_tier(&self, _engine: &TierEngine, files: &mut [File], tiers: &mut [Tier]) {
        log::trace!("simulate_tier: processing {} files", files.len());
        for tier in tiers.iter_mut() {
            tier.sim_usage_bytes = 0;
            tier.incoming_files.clear();
        }

        for file in files {
            let mut fitted = false;
            for tier in tiers.iter_mut() {
                if !tier.full_test(file.size) {
                    tier.sim_usage_bytes += file.size;
                    let target_tier_id = tier.id.clone();
                    let target_tier_path = tier.path.to_string_lossy().into_owned();

                    if file.tier_id != target_tier_id {
                        let mut enqueued_file = file.clone();
                        // Keep enqueued_file.metadata.tier_path as the old path (source)
                        enqueued_file.tier_id = target_tier_id.clone();
                        tier.incoming_files.push(enqueued_file);
                    }

                    file.metadata.tier_path = target_tier_path;
                    file.tier_id = target_tier_id;
                    fitted = true;
                    break;
                }
            }
            if !fitted {
                log::error!("Could not fit file in any tiers: {:?}", file.relative_path);
            }
        }
    }

    fn move_files(&self, engine: &TierEngine, tiers: &mut [Tier]) {
        log::trace!("move_files: starting transfer threads");
        thread::scope(|s| {
            for tier in tiers.iter_mut() {
                let buff_sz = engine.config.copy_buff_sz;
                let run_path = &engine.config.run_path;
                let db_path = &engine.db_path;
                s.spawn(move || {
                    tier.transfer_files(buff_sz, run_path, db_path);
                });
            }
        });
    }
}
