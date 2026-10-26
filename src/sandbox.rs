//! The sandbox launcher every AP helper goes through (design doc §4).
//!
//! AP helpers import worlds — i.e. run arbitrary apworld Python — so each one
//! is started through [`Sandbox::command`], which applies the strongest tier
//! detected at startup:
//!
//! - **bubblewrap** (strong): fresh user/pid/ipc/uts/cgroup namespaces, no
//!   network unless the job needs it, a read-only view of just the system
//!   libraries + the AP install + the scripts, only the job's own paths
//!   writable, a private `/tmp`, all capabilities dropped.
//! - **limits** (the universal floor): a separate session, resource limits,
//!   and a wall-clock timeout that kills the whole process group.
//!
//! Both tiers start the child with an empty environment plus only what the job
//! passes — Bifrost's own secrets (AP passwords, proxy secret) never reach
//! world code. Resource limits and timeouts apply on both tiers.
//!
//! Custom (uploaded) apworlds need the strong tier unless the operator opts in
//! (`BIFROST_ALLOW_UNSANDBOXED_CUSTOM_WORLDS=1`); see [`Sandbox::custom_worlds_allowed`].

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};
use std::sync::OnceLock;
use std::time::Duration;

use tokio::process::Command;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Tier {
    Bubblewrap,
    Limits,
}

/// Per-process resource ceilings (both tiers).
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// Address-space cap. Generation of a large multiworld needs GBs.
    pub mem_bytes: u64,
    /// CPU-time cap (the wall-clock timeout is per job).
    pub cpu_secs: u64,
    /// Largest file the job may write.
    pub fsize_bytes: u64,
}

pub struct Sandbox {
    tier: Tier,
    bwrap: Option<PathBuf>,
    limits: Limits,
    allow_unsandboxed_custom: bool,
    /// Why bubblewrap isn't in use (for the posture line), if it isn't.
    weak_reason: Option<String>,
}

/// One sandboxed run.
pub struct Job<'a> {
    pub program: &'a Path,
    pub args: Vec<OsString>,
    pub cwd: &'a Path,
    /// The child's whole environment (besides PATH/HOME/LANG).
    pub env: Vec<(&'static str, OsString)>,
    /// Extra read-only paths (beyond system libs).
    pub read: Vec<PathBuf>,
    /// Writable paths.
    pub write: Vec<PathBuf>,
    /// Needs the network (in-logic talks to the AP server).
    pub network: bool,
}

const SAFE_PATH: &str = "/usr/local/bin:/usr/bin:/bin";

impl Sandbox {
    /// Pick the tier from `BIFROST_SANDBOX` (`auto` default, `bwrap`, `none`)
    /// and what works on this host. `probe` is an interpreter to test-run
    /// inside bubblewrap (falls back to `true`).
    pub async fn detect(probe: &Path) -> Sandbox {
        let var = crate::config::var;
        let want = var("BIFROST_SANDBOX").unwrap_or_else(|| "auto".into());
        let mb = |k: &str, d: u64| var(k).and_then(|v| v.parse().ok()).unwrap_or(d);
        let limits = Limits {
            mem_bytes: mb("BIFROST_SANDBOX_MEM_MB", 4096) << 20,
            cpu_secs: mb("BIFROST_SANDBOX_CPU_SECS", 1800),
            fsize_bytes: mb("BIFROST_SANDBOX_FILE_MB", 1024) << 20,
        };
        let allow_unsandboxed_custom =
            var("BIFROST_ALLOW_UNSANDBOXED_CUSTOM_WORLDS").as_deref() == Some("1");
        let mut sb = Sandbox {
            tier: Tier::Limits,
            bwrap: None,
            limits,
            allow_unsandboxed_custom,
            weak_reason: None,
        };
        if want == "none" {
            sb.weak_reason = Some("disabled by BIFROST_SANDBOX=none".into());
            return sb;
        }
        let bwrap = var("BIFROST_BWRAP")
            .map(PathBuf::from)
            .or_else(|| find_in_path("bwrap"));
        let Some(bwrap) = bwrap else {
            if want == "bwrap" {
                panic!("BIFROST_SANDBOX=bwrap but bubblewrap (bwrap) isn't installed");
            }
            sb.weak_reason = Some("bubblewrap is not installed".into());
            return sb;
        };
        sb.bwrap = Some(bwrap);
        sb.tier = Tier::Bubblewrap;
        match sb.probe(probe).await {
            Ok(()) => sb,
            Err(e) => {
                if want == "bwrap" {
                    panic!("BIFROST_SANDBOX=bwrap but bubblewrap doesn't work here: {e}");
                }
                sb.tier = Tier::Limits;
                sb.bwrap = None;
                sb.weak_reason = Some(format!("bubblewrap doesn't work here ({e})"));
                sb
            }
        }
    }

    async fn probe(&self, interp: &Path) -> Result<(), String> {
        let (program, args): (&Path, Vec<OsString>) = if interp.exists() {
            (interp, vec!["-c".into(), "pass".into()])
        } else {
            (Path::new("/usr/bin/true"), vec![])
        };
        let job = Job {
            program,
            args,
            cwd: Path::new("/"),
            env: vec![],
            read: vec![],
            write: vec![],
            network: false,
        };
        let out = run(self.command(&job), Duration::from_secs(20)).await?;
        if out.status.success() {
            Ok(())
        } else {
            let err = String::from_utf8_lossy(&out.stderr);
            Err(err.lines().last().unwrap_or("failed").trim().to_string())
        }
    }

    pub fn strong(&self) -> bool {
        self.tier == Tier::Bubblewrap
    }

    /// May untrusted (uploaded) apworlds run here?
    pub fn custom_worlds_allowed(&self) -> bool {
        self.strong() || self.allow_unsandboxed_custom
    }

    /// One line for the sysadmin: what protects the host from world code.
    pub fn posture(&self) -> String {
        match self.tier {
            Tier::Bubblewrap => "strong — bubblewrap: no network for generation/checks, \
                                 read-only system, only the season's files writable"
                .into(),
            Tier::Limits => {
                let why = self
                    .weak_reason
                    .as_deref()
                    .unwrap_or("no namespace sandbox");
                let custom = if self.allow_unsandboxed_custom {
                    "custom apworlds ALLOWED anyway (BIFROST_ALLOW_UNSANDBOXED_CUSTOM_WORLDS=1)"
                } else {
                    "custom apworlds are refused — install bubblewrap or use the container image"
                };
                format!("minimal — resource limits + timeouts only ({why}); {custom}")
            }
        }
    }

    /// Build the command for `job` on this tier. Run it with [`run`].
    pub fn command(&self, job: &Job) -> Command {
        self.build(job, true)
    }

    /// Like [`command`](Self::command) for a long-running service (the AP
    /// server): same jail and limits, but no CPU-time cap — a server that runs
    /// for weeks would eventually exhaust any fixed one.
    pub fn service(&self, job: &Job) -> Command {
        self.build(job, false)
    }

    fn build(&self, job: &Job, cpu_cap: bool) -> Command {
        let mut cmd = match (&self.tier, &self.bwrap) {
            (Tier::Bubblewrap, Some(bwrap)) => {
                let mut c = Command::new(bwrap);
                c.args(bwrap_args(job));
                c.arg("--").arg(job.program).args(&job.args);
                c
            }
            _ => {
                let mut c = Command::new(job.program);
                c.args(&job.args).current_dir(job.cwd);
                c
            }
        };
        cmd.env_clear()
            .env("PATH", SAFE_PATH)
            .env("HOME", "/tmp")
            .env("LANG", "C.UTF-8")
            .envs(job.env.iter().map(|(k, v)| (*k, v)))
            .stdin(Stdio::null())
            .kill_on_drop(true);
        let limits = self.limits;
        // SAFETY: only async-signal-safe libc calls between fork and exec.
        unsafe {
            cmd.pre_exec(move || {
                // Own session/process group, so a timeout can kill everything
                // the job spawned.
                libc::setsid();
                set_limit(libc::RLIMIT_AS, limits.mem_bytes);
                if cpu_cap {
                    set_limit(libc::RLIMIT_CPU, limits.cpu_secs);
                }
                set_limit(libc::RLIMIT_FSIZE, limits.fsize_bytes);
                set_limit(libc::RLIMIT_CORE, 0);
                Ok(())
            });
        }
        cmd
    }
}

fn set_limit(resource: libc::__rlimit_resource_t, value: u64) {
    let lim = libc::rlimit {
        rlim_cur: value as libc::rlim_t,
        rlim_max: value as libc::rlim_t,
    };
    // SAFETY: plain syscall on a stack value.
    unsafe {
        libc::setrlimit(resource, &lim);
    }
}

/// Run a sandbox command to completion, killing its whole process group if it
/// outlives `timeout`.
pub async fn run(mut cmd: Command, timeout: Duration) -> Result<Output, String> {
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let child = cmd.spawn().map_err(|e| format!("spawn: {e}"))?;
    let pid = child.id();
    match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(r) => r.map_err(|e| format!("wait: {e}")),
        Err(_) => {
            // The child is dropped (kill_on_drop); take its group down too.
            if let Some(pid) = pid {
                // SAFETY: signalling a process group we created via setsid.
                unsafe {
                    libc::killpg(pid as libc::pid_t, libc::SIGKILL);
                }
            }
            Err(format!("timed out after {} s", timeout.as_secs()))
        }
    }
}

/// bubblewrap arguments for `job` (everything before `--`).
fn bwrap_args(job: &Job) -> Vec<OsString> {
    let mut a: Vec<OsString> = Vec::new();
    let mut push = |xs: &[&std::ffi::OsStr]| a.extend(xs.iter().map(|x| x.to_os_string()));
    push(&[
        "--die-with-parent".as_ref(),
        "--new-session".as_ref(),
        "--unshare-all".as_ref(),
        "--cap-drop".as_ref(),
        "ALL".as_ref(),
    ]);
    if job.network {
        push(&["--share-net".as_ref()]);
    }
    push(&["--ro-bind".as_ref(), "/usr".as_ref(), "/usr".as_ref()]);
    // Merged-/usr systems link these into /usr; others have real dirs.
    for top in ["/bin", "/sbin", "/lib", "/lib32", "/lib64", "/libx32"] {
        let p = Path::new(top);
        match std::fs::read_link(p) {
            Ok(target) => push(&["--symlink".as_ref(), target.as_os_str(), p.as_os_str()]),
            Err(_) if p.is_dir() => push(&["--ro-bind".as_ref(), p.as_os_str(), p.as_os_str()]),
            Err(_) => {}
        }
    }
    // Just the /etc a Python process needs — never all of /etc (service
    // drop-ins there can hold secrets).
    let mut etc = vec![
        "/etc/ld.so.cache",
        "/etc/localtime",
        "/etc/alternatives",
        "/etc/passwd",
        "/etc/group",
        "/etc/nsswitch.conf",
        "/etc/ssl",
        "/etc/ca-certificates",
    ];
    if job.network {
        etc.extend(["/etc/hosts", "/etc/resolv.conf"]);
    }
    for e in etc {
        push(&["--ro-bind-try".as_ref(), e.as_ref(), e.as_ref()]);
    }
    push(&[
        "--proc".as_ref(),
        "/proc".as_ref(),
        "--dev".as_ref(),
        "/dev".as_ref(),
        "--tmpfs".as_ref(),
        "/tmp".as_ref(),
    ]);
    // The interpreter's own tree (a venv may live outside /usr).
    for p in interpreter_roots(job.program) {
        push(&["--ro-bind-try".as_ref(), p.as_os_str(), p.as_os_str()]);
    }
    for p in &job.read {
        push(&["--ro-bind-try".as_ref(), p.as_os_str(), p.as_os_str()]);
    }
    for p in &job.write {
        push(&["--bind-try".as_ref(), p.as_os_str(), p.as_os_str()]);
    }
    push(&["--chdir".as_ref(), job.cwd.as_os_str()]);
    a
}

/// Directories that must be visible for `program` to run: its venv/prefix
/// (`…/bin/python` → `…`) and that of the real interpreter it links to.
fn interpreter_roots(program: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let prefix = |p: &Path| p.parent().and_then(Path::parent).map(Path::to_path_buf);
    if let Some(p) = prefix(program) {
        out.push(p);
    }
    if let Ok(real) = std::fs::canonicalize(program)
        && let Some(p) = prefix(&real)
        && !out.contains(&p)
    {
        out.push(p);
    }
    // /usr is already bound read-only; binding "/" would expose everything.
    out.retain(|p| p != Path::new("/") && !p.starts_with("/usr"));
    out
}

fn find_in_path(name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH")
        .into_iter()
        .flat_map(|p| std::env::split_paths(&p).collect::<Vec<_>>())
        .chain(["/usr/bin".into(), "/usr/local/bin".into()])
        .map(|d| d.join(name))
        .find(|p| p.is_file())
}

static SANDBOX: OnceLock<Sandbox> = OnceLock::new();

pub fn init(sb: Sandbox) -> &'static Sandbox {
    if SANDBOX.set(sb).is_err() {
        panic!("sandbox::init called twice");
    }
    get()
}

pub fn get() -> &'static Sandbox {
    SANDBOX.get().expect("sandbox not initialised")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits_only() -> Sandbox {
        Sandbox {
            tier: Tier::Limits,
            bwrap: None,
            limits: Limits {
                mem_bytes: 512 << 20,
                cpu_secs: 10,
                fsize_bytes: 1 << 20,
            },
            allow_unsandboxed_custom: false,
            weak_reason: None,
        }
    }

    fn sh(script: &str) -> Job<'static> {
        Job {
            program: Path::new("/bin/sh"),
            args: vec!["-c".into(), script.into()],
            cwd: Path::new("/"),
            env: vec![("ONLY_THIS", "1".into())],
            read: vec![],
            write: vec![],
            network: false,
        }
    }

    #[tokio::test]
    async fn environment_is_scrubbed() {
        // SAFETY: test-only; no other thread reads this variable.
        unsafe { std::env::set_var("BIFROST_AP_PASSWORD", "hunter2") };
        let sb = limits_only();
        let out = run(sb.command(&sh("env")), Duration::from_secs(5))
            .await
            .unwrap();
        let env = String::from_utf8_lossy(&out.stdout);
        assert!(!env.contains("hunter2"));
        assert!(env.contains("ONLY_THIS=1"));
    }

    #[tokio::test]
    async fn timeout_kills_the_process_group() {
        let sb = limits_only();
        // A grandchild that would outlive a naive kill of the direct child.
        let marker = std::env::temp_dir().join(format!("bifrost-sbx-{}", std::process::id()));
        let _ = std::fs::remove_file(&marker);
        let script = format!("(sleep 2; touch {}) & sleep 30", marker.display());
        let r = run(sb.command(&sh(&script)), Duration::from_millis(300)).await;
        assert!(r.unwrap_err().contains("timed out"));
        tokio::time::sleep(Duration::from_millis(2500)).await;
        assert!(!marker.exists(), "grandchild survived the timeout");
    }

    #[tokio::test]
    async fn file_size_limit_applies() {
        let sb = limits_only();
        let f = std::env::temp_dir().join(format!("bifrost-sbx-big-{}", std::process::id()));
        let script = format!(
            "head -c 4000000 /dev/zero > {} 2>/dev/null; echo $?",
            f.display()
        );
        let out = run(sb.command(&sh(&script)), Duration::from_secs(5))
            .await
            .unwrap();
        let size = std::fs::metadata(&f).map(|m| m.len()).unwrap_or(0);
        let _ = std::fs::remove_file(&f);
        assert!(size <= 1 << 20, "wrote {size} bytes past a 1 MiB cap");
        assert_ne!(String::from_utf8_lossy(&out.stdout).trim(), "0");
    }

    #[test]
    fn bwrap_never_binds_root_or_all_of_etc() {
        let job = Job {
            program: Path::new("/usr/bin/python3"),
            args: vec![],
            cwd: Path::new("/data/seasons/current"),
            env: vec![],
            read: vec!["/opt/archipelago".into()],
            write: vec!["/data/seasons/current/output".into()],
            network: false,
        };
        let a: Vec<String> = bwrap_args(&job)
            .iter()
            .map(|s| s.to_string_lossy().into_owned())
            .collect();
        let binds: Vec<&[String]> = a.windows(3).filter(|w| w[0].contains("bind")).collect();
        assert!(binds.iter().all(|w| w[1] != "/" && w[1] != "/etc"), "{a:?}");
        assert!(a.contains(&"--unshare-all".to_string()));
        assert!(!a.contains(&"--share-net".to_string()));
        assert!(
            a.windows(3)
                .any(|w| w[0] == "--bind-try" && w[1] == "/data/seasons/current/output")
        );
        assert!(
            a.windows(3)
                .any(|w| w[0] == "--ro-bind-try" && w[1] == "/opt/archipelago")
        );
    }

    /// The strong tier's actual guarantees, attacked from inside. Skips where
    /// bubblewrap isn't usable (it is on Linux CI/dev boxes with userns).
    #[tokio::test]
    async fn bubblewrap_contains_hostile_code() {
        let Some(bwrap) = find_in_path("bwrap") else {
            return;
        };
        let python = Path::new("/usr/bin/python3");
        if !python.exists() {
            return;
        }
        let mut sb = limits_only();
        sb.tier = Tier::Bubblewrap;
        sb.bwrap = Some(bwrap);
        if sb.probe(python).await.is_err() {
            return;
        }
        let base = std::env::temp_dir().join(format!("bifrost-sbx-escape-{}", std::process::id()));
        let (secret, ro, rw) = (base.join("secret"), base.join("ro"), base.join("rw"));
        for d in [&secret, &ro, &rw] {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::write(secret.join("bifrost.db"), "password hashes").unwrap();
        std::fs::write(ro.join("world.apworld"), "trusted").unwrap();
        // SAFETY: test-only; no other thread reads this variable.
        unsafe { std::env::set_var("BIFROST_FWD_SECRET", "s3cret") };
        let script = format!(
            r#"
import json, os, socket
r = {{}}
def tryit(name, f):
    try:
        f(); r[name] = "ALLOWED"
    except Exception as e:
        r[name] = "blocked"
tryit("net", lambda: socket.create_connection(("1.1.1.1", 53), timeout=2))
tryit("read_secret", lambda: open("{secret}/bifrost.db").read())
tryit("write_ro", lambda: open("{ro}/world.apworld", "w").write("evil"))
tryit("write_rw", lambda: open("{rw}/out.txt", "w").write("ok"))
tryit("etc_systemd", lambda: os.listdir("/etc/systemd"))
tryit("home", lambda: os.listdir("/home"))
r["env_secret"] = "LEAKED" if "s3cret" in json.dumps(dict(os.environ)) else "clean"
print(json.dumps(r))
"#,
            secret = secret.display(),
            ro = ro.display(),
            rw = rw.display()
        );
        let job = Job {
            program: python,
            args: vec!["-c".into(), script.into()],
            cwd: &rw,
            env: vec![],
            read: vec![ro.clone()],
            write: vec![rw.clone()],
            network: false,
        };
        let out = run(sb.command(&job), Duration::from_secs(30))
            .await
            .unwrap();
        let _ = std::fs::remove_dir_all(&base);
        let r: serde_json::Value = serde_json::from_slice(&out.stdout)
            .unwrap_or_else(|_| panic!("{}", String::from_utf8_lossy(&out.stderr)));
        assert_eq!(r["net"], "blocked", "{r}");
        assert_eq!(r["read_secret"], "blocked", "{r}");
        assert_eq!(r["write_ro"], "blocked", "{r}");
        assert_eq!(r["write_rw"], "ALLOWED", "{r}");
        assert_eq!(r["etc_systemd"], "blocked", "{r}");
        assert_eq!(r["home"], "blocked", "{r}");
        assert_eq!(r["env_secret"], "clean", "{r}");
    }
}
