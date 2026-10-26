//! Live load, for the sysadmin (settings page): the host's memory — or the
//! container's, when it has a memory limit — CPU and data-disk space, and
//! what Bifrost's own processes use: each live season's AP server (its whole
//! process tree: bubblewrap + MultiServer) and the sandboxed helpers (checks,
//! generations, in-logic). Sampled every 10 s from `/proc`, an hour kept.
//!
//! Also the warnings drawn from it — tight memory, a busy CPU, a filling
//! disk — and whether another live season would fit (the season admin page
//! shows that before generating).

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

const EVERY: Duration = Duration::from_secs(10);
/// An hour of samples.
const KEEP: usize = 360;
const MIB: u64 = 1 << 20;

#[derive(Clone, Debug, Default)]
pub struct Sample {
    pub at: i64,
    pub mem_used: u64,
    pub mem_total: u64,
    /// Host CPU busy, % of all cores.
    pub cpu: f32,
    /// Live season → its AP server's (resident bytes, CPU % of one core).
    pub servers: BTreeMap<i64, (u64, f32)>,
    /// Bifrost's other child processes right now — sandboxed helpers, release
    /// installs, archive tar: (resident bytes, CPU % of one core, count).
    pub helpers: (u64, f32, usize),
    /// Bifrost itself: (resident bytes, CPU % of one core).
    pub bifrost: (u64, f32),
}

#[derive(Clone, Debug, Default)]
pub struct Load {
    /// Oldest first.
    pub history: VecDeque<Sample>,
    pub disk_free: u64,
    pub disk_total: u64,
    pub cores: usize,
    /// Memory is the container's cgroup limit, not the host's.
    pub cgroup: bool,
}

impl Load {
    pub fn latest(&self) -> Option<&Sample> {
        self.history.back()
    }
}

static LOAD: Mutex<Option<Load>> = Mutex::new(None);

/// The samples so far (`None` until the first one).
pub fn snapshot() -> Option<Load> {
    LOAD.lock().unwrap().clone()
}

/// Sample forever (Linux `/proc`; elsewhere there's simply no load panel).
pub fn spawn_sampler(data_dir: std::path::PathBuf) {
    if !Path::new("/proc/self/stat").exists() {
        return;
    }
    tokio::spawn(async move {
        let mut sampler = Sampler::default();
        let mut tick = tokio::time::interval(EVERY);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            let dir = data_dir.clone();
            let s = std::mem::take(&mut sampler);
            let Ok((s, sample, disk)) = tokio::task::spawn_blocking(move || {
                let mut s = s;
                let sample = s.sample();
                (s, sample, disk_space(&dir))
            })
            .await
            else {
                continue;
            };
            sampler = s;
            let mut guard = LOAD.lock().unwrap();
            let load = guard.get_or_insert_with(Load::default);
            if let Some(sample) = sample {
                load.cgroup = sampler.cgroup;
                load.cores = sampler.cpus;
                load.history.push_back(sample);
                while load.history.len() > KEEP {
                    load.history.pop_front();
                }
            }
            (load.disk_free, load.disk_total) = disk.unwrap_or_default();
        }
    });
}

/// Things the sysadmin should know about now.
pub fn warnings(l: &Load) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(s) = l.latest() {
        let free = s.mem_total.saturating_sub(s.mem_used);
        if s.mem_total > 0 && free < (s.mem_total / 10).max(512 * MIB) {
            out.push(format!(
                "Memory is tight: {} MiB of {} MiB free. A live season whose server runs out is killed.",
                free / MIB,
                s.mem_total / MIB
            ));
        }
    }
    let recent: Vec<f32> = l.history.iter().rev().take(30).map(|s| s.cpu).collect();
    if recent.len() == 30 && recent.iter().sum::<f32>() / 30.0 > 90.0 {
        out.push("The CPU has been over 90% busy for the last 5 minutes.".into());
    }
    if l.disk_total > 0 && l.disk_free < (l.disk_total / 20).max(1024 * MIB) {
        out.push(format!(
            "The data disk is nearly full: {} MiB free.",
            l.disk_free / MIB
        ));
    }
    out
}

/// Roughly what one more live season's server needs: the largest running
/// one, else a typical small one.
fn per_season(s: &Sample) -> u64 {
    s.servers.values().map(|(rss, _)| *rss).max().unwrap_or(0).max(256 * MIB)
}

/// A warning when another live season probably wouldn't fit in memory.
pub fn room_for_another_season() -> Option<String> {
    let l = snapshot()?;
    let s = l.latest()?;
    let need = per_season(s);
    let free = s.mem_total.saturating_sub(s.mem_used);
    (free < need + 256 * MIB).then(|| {
        format!(
            "Memory is tight: {} MiB free, and a live season's server here uses about {} MiB. \
             Starting another may get a server killed — ask a sysadmin to check the load.",
            free / MIB,
            need / MIB
        )
    })
}

// ---- sampling ----

#[derive(Default)]
struct Sampler {
    /// pid → CPU ticks at the last sample.
    ticks: HashMap<u32, u64>,
    /// Host (busy, total) jiffies at the last sample.
    host: Option<(u64, u64)>,
    /// When the last sample was taken (per-process CPU is ticks over this).
    last: Option<std::time::Instant>,
    /// The host's CPU count.
    cpus: usize,
    cgroup: bool,
}

struct Proc {
    ppid: u32,
    ticks: u64,
    rss: u64,
}

impl Sampler {
    fn sample(&mut self) -> Option<Sample> {
        let (mem_used, mem_total, cgroup) = memory()?;
        self.cgroup = cgroup;
        let ((busy, total), cpus) = host_cpu()?;
        self.cpus = cpus;
        let cpu = match self.host.replace((busy, total)) {
            Some((b0, t0)) if total > t0 => (busy - b0) as f32 * 100.0 / (total - t0) as f32,
            _ => 0.0,
        };
        let procs = processes();
        let tck = clock_ticks();
        let now = std::time::Instant::now();
        let secs = self.last.replace(now).map_or(EVERY.as_secs_f32(), |t| now.duration_since(t).as_secs_f32()).max(0.1);
        let pct = |pid: u32, p: &Proc, prev: &HashMap<u32, u64>| match prev.get(&pid) {
            Some(&t0) if p.ticks >= t0 => (p.ticks - t0) as f32 * 100.0 / (tck * secs),
            _ => 0.0,
        };
        let me = std::process::id();
        let server_pids: HashMap<u32, i64> =
            crate::apserver::server_pids().into_iter().map(|(season, pid)| (pid, season)).collect();
        let mut servers: BTreeMap<i64, (u64, f32)> =
            server_pids.values().map(|&s| (s, (0, 0.0))).collect();
        let mut helpers = (0u64, 0f32, 0usize);
        let mut bifrost = (0u64, 0f32);
        let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
        for (&pid, p) in &procs {
            children.entry(p.ppid).or_default().push(pid);
        }
        if let Some(p) = procs.get(&me) {
            bifrost = (p.rss, pct(me, p, &self.ticks));
        }
        // Everything Bifrost started, by ancestry: a server and all under it
        // (bubblewrap's children start sessions of their own) are its season's;
        // the rest are helpers, each direct child of Bifrost one job.
        let mut stack: Vec<(u32, Option<i64>)> =
            children.get(&me).into_iter().flatten().map(|&pid| (pid, None)).collect();
        while let Some((pid, owner)) = stack.pop() {
            let Some(p) = procs.get(&pid) else { continue };
            let owner = owner.or_else(|| server_pids.get(&pid).copied());
            let (rss, c) = (p.rss, pct(pid, p, &self.ticks));
            match owner {
                Some(season) => {
                    let e = servers.entry(season).or_default();
                    e.0 += rss;
                    e.1 += c;
                }
                None => {
                    helpers.0 += rss;
                    helpers.1 += c;
                    if p.ppid == me {
                        helpers.2 += 1;
                    }
                }
            }
            stack.extend(children.get(&pid).into_iter().flatten().map(|&kid| (kid, owner)));
        }
        self.ticks = procs.iter().map(|(&pid, p)| (pid, p.ticks)).collect();
        Some(Sample { at: crate::auth::now(), mem_used, mem_total, cpu, servers, helpers, bifrost })
    }
}

fn clock_ticks() -> f32 {
    (unsafe { libc::sysconf(libc::_SC_CLK_TCK) }).max(1) as f32
}

fn page_size() -> u64 {
    (unsafe { libc::sysconf(libc::_SC_PAGESIZE) }).max(1) as u64
}

/// Every process: parent, CPU ticks, resident bytes.
fn processes() -> HashMap<u32, Proc> {
    let page = page_size();
    std::fs::read_dir("/proc")
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let pid: u32 = e.file_name().to_str()?.parse().ok()?;
            let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
            parse_stat(&stat, page).map(|p| (pid, p))
        })
        .collect()
}

/// `/proc/<pid>/stat`: the fields after `(comm)` — which may itself hold
/// spaces and parentheses — are state, ppid, …, utime (14th overall),
/// stime (15th), …, rss in pages (24th).
fn parse_stat(stat: &str, page: u64) -> Option<Proc> {
    let rest = &stat[stat.rfind(')')? + 1..];
    let f: Vec<&str> = rest.split_whitespace().collect();
    let num = |i: usize| f.get(i)?.parse::<u64>().ok();
    Some(Proc {
        ppid: num(1)? as u32,
        ticks: num(11)? + num(12)?,
        rss: num(21)? * page,
    })
}

/// (used, total, from a cgroup limit). The nearest cgroup v2 memory limit
/// over Bifrost — a container's, or a systemd unit's `MemoryMax=` — wins over
/// the host's totals; page cache it could drop doesn't count as used.
fn memory() -> Option<(u64, u64, bool)> {
    let read = |p: &Path| std::fs::read_to_string(p).ok();
    if let Some(cg) = limited_cgroup() {
        let max: u64 = read(&cg.join("memory.max"))?.trim().parse().ok()?;
        let current: u64 = read(&cg.join("memory.current"))?.trim().parse().ok()?;
        let inactive = read(&cg.join("memory.stat"))
            .and_then(|s| field(&s, "inactive_file"))
            .unwrap_or(0);
        return Some((current.saturating_sub(inactive), max, true));
    }
    let info = read(Path::new("/proc/meminfo"))?;
    let kib = |k: &str| field(&info, k).map(|v| v * 1024);
    let total = kib("MemTotal:")?;
    Some((total.saturating_sub(kib("MemAvailable:")?), total, false))
}

/// The closest cgroup (v2) at or above Bifrost's own that sets a memory limit.
fn limited_cgroup() -> Option<std::path::PathBuf> {
    let own = std::fs::read_to_string("/proc/self/cgroup").ok()?;
    let rel = own.lines().find_map(|l| l.strip_prefix("0::"))?.trim().trim_start_matches('/');
    let root = Path::new("/sys/fs/cgroup");
    let mut dir = root.join(rel);
    loop {
        let limited = std::fs::read_to_string(dir.join("memory.max"))
            .is_ok_and(|m| m.trim().parse::<u64>().is_ok());
        if limited {
            return Some(dir);
        }
        if dir == root || !dir.pop() || !dir.starts_with(root) {
            return None;
        }
    }
}

/// The number after `key` at the start of a line.
fn field(text: &str, key: &str) -> Option<u64> {
    text.lines()
        .find_map(|l| l.strip_prefix(key)?.split_whitespace().next()?.parse().ok())
}

/// Host (busy, total) jiffies from `/proc/stat`'s first line, and how many
/// CPUs that line covers (the host's, like the percentage).
fn host_cpu() -> Option<((u64, u64), usize)> {
    let stat = std::fs::read_to_string("/proc/stat").ok()?;
    let cpus = stat
        .lines()
        .filter(|l| l.strip_prefix("cpu").is_some_and(|r| r.starts_with(|c: char| c.is_ascii_digit())))
        .count();
    Some((parse_cpu_line(stat.lines().next()?)?, cpus.max(1)))
}

fn parse_cpu_line(line: &str) -> Option<(u64, u64)> {
    let v: Vec<u64> = line.strip_prefix("cpu ")?.split_whitespace().filter_map(|x| x.parse().ok()).collect();
    // user nice system idle iowait irq softirq steal (guest time is in user)
    let total: u64 = v.iter().take(8).sum();
    let idle = v.get(3)? + v.get(4).unwrap_or(&0);
    Some((total - idle, total))
}

/// (free, total) bytes on the data dir's filesystem.
fn disk_space(dir: &Path) -> Option<(u64, u64)> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(dir.as_os_str().as_bytes()).ok()?;
    let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
    (unsafe { libc::statvfs(c.as_ptr(), &mut s) } == 0).then(|| {
        let f = s.f_frsize as u64;
        (s.f_bavail as u64 * f, s.f_blocks as u64 * f)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stat_lines_parse_even_with_odd_names() {
        let line = "1234 (python3 (x) y) S 1 1234 1234 0 -1 4194560 100 0 0 0 250 50 0 0 20 0 3 0 99 123456789 2048 18446744073709551615";
        let p = parse_stat(line, 4096).unwrap();
        assert_eq!((p.ppid, p.ticks, p.rss), (1, 300, 2048 * 4096));
        assert!(parse_stat("garbage", 4096).is_none());
    }

    #[test]
    fn host_cpu_counts_idle_and_iowait_as_idle() {
        assert_eq!(parse_cpu_line("cpu  100 0 50 800 50 0 0 0 0 0"), Some((150, 1000)));
        assert_eq!(parse_cpu_line("cpu0 1 2 3 4"), None);
    }

    #[test]
    fn warnings_fire_on_tight_memory_busy_cpu_and_full_disk() {
        let s = |used: u64, cpu: f32| Sample { mem_used: used, mem_total: 8192 * MIB, cpu, ..Sample::default() };
        let mut l = Load { disk_free: 50_000 * MIB, disk_total: 100_000 * MIB, cores: 4, ..Load::default() };
        l.history.extend((0..30).map(|_| s(1024 * MIB, 20.0)));
        assert!(warnings(&l).is_empty());
        l.history.extend((0..30).map(|_| s(7900 * MIB, 95.0)));
        l.disk_free = 500 * MIB;
        let w = warnings(&l);
        assert_eq!(w.len(), 3, "{w:?}");
    }

    #[test]
    fn a_season_needs_at_least_what_the_biggest_server_uses() {
        let mut s = Sample::default();
        assert_eq!(per_season(&s), 256 * MIB);
        s.servers.insert(1, (900 * MIB, 1.0));
        s.servers.insert(2, (300 * MIB, 1.0));
        assert_eq!(per_season(&s), 900 * MIB);
    }

    #[test]
    fn this_process_is_seen() {
        let mut sampler = Sampler::default();
        let s = sampler.sample().unwrap();
        assert!(s.mem_total > 0 && s.bifrost.0 > 0, "{s:?}");
    }
}
