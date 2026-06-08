//! Constants and functions for EMA (Exponential Moving Average) popularity calculations.

pub const MINUTE: f64 = 60.0;
pub const HOUR: f64 = 60.0 * MINUTE;
pub const DAY: f64 = 24.0 * HOUR;
pub const WEEK: f64 = 7.0 * DAY;

pub const START_DAMPING: f64 = 50000.0;
pub const DAMPING: f64 = 1000000.0;
pub const MULTIPLIER: f64 = 3600.0;
pub const REACH_FULL_DAMPING_AFTER: f64 = 1.0 * WEEK;
pub const SLOPE: f64 = (DAMPING - START_DAMPING) / REACH_FULL_DAMPING_AFTER;

pub const AVG_USAGE: f64 = 0.238; // 40hr/(7days * 24hr/day)

/// Calculates the next popularity value based on EMA formula:
/// `y[n] = MULTIPLIER * (accesses / period) / damping + (1.0 - 1.0 / damping) * y[n-1]`
pub fn calculate_popularity(
    current_popularity: f64,
    accesses: u64,
    period_seconds: f64,
    file_age_seconds: f64,
) -> f64 {
    // Determine the damping factor based on how long the file has been tracked
    let current_damping = if file_age_seconds < REACH_FULL_DAMPING_AFTER {
        START_DAMPING + SLOPE * file_age_seconds
    } else {
        DAMPING
    };

    let usage_frequency = accesses as f64 / period_seconds;
    let next_popularity = (MULTIPLIER * usage_frequency / current_damping)
        + (1.0 - 1.0 / current_damping) * current_popularity;

    next_popularity
}
