//! Timing helpers for the H-06 performance probes
//! (docs/research/h-06-performance.md): wall and on-CPU time per call,
//! percentiles, and the process's memory. Linux only; elsewhere CPU time
//! and memory read as zero.

use std::time::{Duration, Instant};

/// Wall and on-CPU time of one call.
#[derive(Debug, Clone, Copy)]
pub struct Sample {
    pub wall: Duration,
    pub cpu: Duration,
}

/// Percentiles of repeated calls, in milliseconds.
#[derive(Debug, Clone, Copy)]
pub struct Stats {
    pub n: usize,
    pub wall_p50: f64,
    pub wall_p95: f64,
    pub wall_max: f64,
    pub cpu_p50: f64,
    pub cpu_p95: f64,
}

impl std::fmt::Display for Stats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "n={} wall p50 {:.2} ms p95 {:.2} ms max {:.2} ms | cpu p50 {:.2} ms p95 {:.2} ms",
            self.n, self.wall_p50, self.wall_p95, self.wall_max, self.cpu_p50, self.cpu_p95
        )
    }
}

/// On-CPU time of every thread of this process so far, from each task's
/// `schedstat`: the sum covers an async runtime's workers, which do the
/// work of a call handed to them. Threads that already ended are not
/// counted.
pub fn process_cpu() -> Duration {
    let mut ns = 0u64;
    for task in std::fs::read_dir("/proc/self/task")
        .into_iter()
        .flatten()
        .flatten()
    {
        let Ok(stat) = std::fs::read_to_string(task.path().join("schedstat")) else {
            continue;
        };
        ns += (stat.split_whitespace().next())
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(0);
    }
    Duration::from_nanos(ns)
}

/// One call's result, wall time and process CPU time.
pub fn sample<R>(f: impl FnOnce() -> R) -> (R, Sample) {
    let (cpu0, t0) = (process_cpu(), Instant::now());
    let r = f();
    let wall = t0.elapsed();
    let cpu = process_cpu().saturating_sub(cpu0);
    (r, Sample { wall, cpu })
}

pub fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    let i = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[i.min(sorted.len() - 1)]
}

/// The percentiles of `samples`; panics when there are none.
pub fn stats(samples: &[Sample]) -> Stats {
    let mut wall: Vec<f64> = samples.iter().map(|s| ms(s.wall)).collect();
    let mut cpu: Vec<f64> = samples.iter().map(|s| ms(s.cpu)).collect();
    wall.sort_by(f64::total_cmp);
    cpu.sort_by(f64::total_cmp);
    Stats {
        n: samples.len(),
        wall_p50: percentile(&wall, 0.5),
        wall_p95: percentile(&wall, 0.95),
        wall_max: wall[wall.len() - 1],
        cpu_p50: percentile(&cpu, 0.5),
        cpu_p95: percentile(&cpu, 0.95),
    }
}

/// `warmup` unmeasured calls of `f`, then `n` measured ones.
pub fn measure(warmup: usize, n: usize, mut f: impl FnMut()) -> Stats {
    for _ in 0..warmup {
        f();
    }
    let samples: Vec<Sample> = (0..n).map(|_| sample(&mut f).1).collect();
    stats(&samples)
}

/// The repetition count: `DWD_H06_ITERS`, else `default`.
pub fn iters(default: usize) -> usize {
    (std::env::var("DWD_H06_ITERS").ok())
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// This process's RSS and PSS, MiB (`/proc/self/smaps_rollup`).
pub fn memory_mib() -> (f64, f64) {
    let rollup = std::fs::read_to_string("/proc/self/smaps_rollup").unwrap_or_default();
    let field = |name: &str| {
        (rollup.lines())
            .find_map(|l| l.strip_prefix(name))
            .and_then(|v| v.split_whitespace().next()?.parse::<f64>().ok())
            .map_or(0.0, |kib| kib / 1024.0)
    };
    (field("Rss:"), field("Pss:"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stats_take_percentiles_of_the_samples() {
        let samples: Vec<Sample> = (1..=100)
            .map(|i| Sample {
                wall: Duration::from_millis(i),
                cpu: Duration::from_millis(i / 2),
            })
            .collect();
        let s = stats(&samples);
        assert_eq!((s.n, s.wall_max), (100, 100.0));
        assert_eq!((s.wall_p50.round(), s.wall_p95), (51.0, 95.0));
        assert_eq!(s.cpu_p95, 47.0);
    }
}
