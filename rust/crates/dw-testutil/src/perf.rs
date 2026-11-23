//! Timing helpers for the H-06 performance probes
//! (docs/research/h-06-performance.md): wall and on-CPU time per call,
//! percentiles, and the process's memory. CPU time is Unix only; memory is
//! Linux only and reads as zero elsewhere.

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

/// This process's on-CPU time so far, every thread's included, from
/// `CLOCK_PROCESS_CPUTIME_ID`: cumulative, so a worker thread that exits
/// during a call still counts. Panics when the clock cannot be read.
#[cfg(unix)]
pub fn process_cpu() -> Duration {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `ts` is a valid, writable timespec for the call's duration.
    let rc = unsafe { libc::clock_gettime(libc::CLOCK_PROCESS_CPUTIME_ID, &mut ts) };
    assert_eq!(
        rc,
        0,
        "clock_gettime(CLOCK_PROCESS_CPUTIME_ID): {}",
        std::io::Error::last_os_error()
    );
    let secs = u64::try_from(ts.tv_sec).expect("process CPU seconds");
    let nanos = u32::try_from(ts.tv_nsec).expect("process CPU nanoseconds");
    Duration::new(secs, nanos)
}

#[cfg(not(unix))]
pub fn process_cpu() -> Duration {
    panic!("process CPU time is measured on Unix only")
}

/// One call's result, wall time and process CPU time.
pub fn sample<R>(f: impl FnOnce() -> R) -> (R, Sample) {
    let (cpu0, t0) = (process_cpu(), Instant::now());
    let r = f();
    let wall = t0.elapsed();
    let cpu = process_cpu()
        .checked_sub(cpu0)
        .expect("process CPU time went backwards");
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

    /// Work done on a thread that has exited by the second reading still
    /// counts (the per-thread sum this replaced dropped it).
    #[cfg(unix)]
    #[test]
    fn process_cpu_keeps_the_time_of_exited_threads() {
        fn thread_cpu() -> Duration {
            let mut ts = libc::timespec {
                tv_sec: 0,
                tv_nsec: 0,
            };
            // SAFETY: `ts` is a valid, writable timespec.
            assert_eq!(
                unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut ts) },
                0
            );
            Duration::new(ts.tv_sec as u64, ts.tv_nsec as u32)
        }
        let spin = Duration::from_millis(50);
        let before = process_cpu();
        std::thread::spawn(move || {
            while thread_cpu() < spin {
                std::hint::black_box(0u64);
            }
        })
        .join()
        .unwrap();
        assert!(process_cpu() - before >= spin);
    }
}
