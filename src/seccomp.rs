//! A syscall filter for sandboxed jobs (classic BPF, for seccomp).
//!
//! Namespaces and dropped capabilities keep world code away from the host's
//! files and network; this narrows what it can ask of the kernel. It refuses
//! (`EPERM`) the calls a Python job has no use for and that widen the kernel's
//! attack surface: new namespaces, mounts, kernel keyrings, BPF, perf,
//! userfaultfd, io_uring, module loading, `ptrace`, and the like. Bubblewrap
//! installs it for the job (`--seccomp`); the limits-only tier installs it
//! itself. Only built for architectures it knows.

/// Allow the call.
const RET_ALLOW: u32 = 0x7fff_0000;
/// Fail the call with this errno.
const RET_ERRNO: u32 = 0x0005_0000;

// Classic BPF opcodes.
const LD_W_ABS: u16 = 0x20; // BPF_LD | BPF_W | BPF_ABS
const JMP_JEQ_K: u16 = 0x05 | 0x10;
const JMP_JGE_K: u16 = 0x05 | 0x30;
const JMP_JSET_K: u16 = 0x05 | 0x40;
const RET_K: u16 = 0x06;

// `struct seccomp_data` offsets.
const OFF_NR: u32 = 0;
const OFF_ARCH: u32 = 4;
/// The low 32 bits of the first argument (little-endian).
const OFF_ARG0_LO: u32 = 16;

/// `clone` flags that make new namespaces.
const CLONE_NEW_NAMESPACES: u32 = 0x0002_0000 // NEWNS
    | 0x0200_0000 // NEWCGROUP
    | 0x0400_0000 // NEWUTS
    | 0x0800_0000 // NEWIPC
    | 0x1000_0000 // NEWUSER
    | 0x2000_0000 // NEWPID
    | 0x4000_0000; // NEWNET

#[cfg(target_arch = "x86_64")]
const AUDIT_ARCH: u32 = 0xc000_003e;
#[cfg(target_arch = "aarch64")]
const AUDIT_ARCH: u32 = 0xc000_00b7;

/// Calls refused outright.
#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
fn refused() -> Vec<libc::c_long> {
    let mut v = vec![
        libc::SYS_unshare,
        libc::SYS_setns,
        libc::SYS_mount,
        libc::SYS_umount2,
        libc::SYS_pivot_root,
        libc::SYS_move_mount,
        libc::SYS_open_tree,
        libc::SYS_fsopen,
        libc::SYS_fsconfig,
        libc::SYS_fsmount,
        libc::SYS_fspick,
        libc::SYS_mount_setattr,
        libc::SYS_keyctl,
        libc::SYS_add_key,
        libc::SYS_request_key,
        libc::SYS_bpf,
        libc::SYS_perf_event_open,
        libc::SYS_userfaultfd,
        libc::SYS_io_uring_setup,
        libc::SYS_io_uring_enter,
        libc::SYS_io_uring_register,
        libc::SYS_ptrace,
        libc::SYS_process_vm_readv,
        libc::SYS_process_vm_writev,
        libc::SYS_kcmp,
        libc::SYS_init_module,
        libc::SYS_finit_module,
        libc::SYS_delete_module,
        libc::SYS_kexec_load,
        libc::SYS_kexec_file_load,
        libc::SYS_reboot,
        libc::SYS_swapon,
        libc::SYS_swapoff,
        libc::SYS_syslog,
        libc::SYS_acct,
        libc::SYS_quotactl,
        libc::SYS_settimeofday,
        libc::SYS_clock_settime,
        libc::SYS_clock_adjtime,
        libc::SYS_adjtimex,
        libc::SYS_open_by_handle_at,
        libc::SYS_name_to_handle_at,
        libc::SYS_personality,
    ];
    #[cfg(target_arch = "x86_64")]
    v.extend([
        libc::SYS_iopl,
        libc::SYS_ioperm,
        libc::SYS_modify_ldt,
        libc::SYS_uselib,
    ]);
    v
}

/// One BPF instruction (`struct sock_filter`).
#[derive(Clone, Copy)]
struct Insn {
    code: u16,
    jt: u8,
    jf: u8,
    k: u32,
}

const fn stmt(code: u16, k: u32) -> Insn {
    Insn {
        code,
        jt: 0,
        jf: 0,
        k,
    }
}

/// The filter program, or `None` on an architecture it isn't built for.
fn program() -> Option<Vec<Insn>> {
    #[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
    {
        // Jumps are filled in once the return instructions' places are known.
        enum To {
            Deny,
            NoSys,
            Allow,
            Next,
        }
        let mut code: Vec<(Insn, To, To)> = Vec::new();
        let plain = |i: Insn| (i, To::Next, To::Next);
        code.push(plain(stmt(LD_W_ABS, OFF_ARCH)));
        // Another ABI (e.g. 32-bit calls on x86_64): refuse everything.
        code.push((stmt(JMP_JEQ_K, AUDIT_ARCH), To::Next, To::Deny));
        code.push(plain(stmt(LD_W_ABS, OFF_NR)));
        #[cfg(target_arch = "x86_64")]
        code.push((stmt(JMP_JGE_K, 0x4000_0000), To::Deny, To::Next)); // x32 calls
        for nr in refused() {
            code.push((stmt(JMP_JEQ_K, nr as u32), To::Deny, To::Next));
        }
        // clone3's flags live in memory a filter can't read: refuse it as
        // unknown, and the C library falls back to clone.
        code.push((
            stmt(JMP_JEQ_K, libc::SYS_clone3 as u32),
            To::NoSys,
            To::Next,
        ));
        code.push((stmt(JMP_JEQ_K, libc::SYS_clone as u32), To::Next, To::Allow));
        code.push(plain(stmt(LD_W_ABS, OFF_ARG0_LO)));
        code.push((stmt(JMP_JSET_K, CLONE_NEW_NAMESPACES), To::Deny, To::Allow));
        let allow = code.len();
        let deny = allow + 1;
        let nosys = allow + 2;
        let mut out: Vec<Insn> = code
            .iter()
            .enumerate()
            .map(|(i, (insn, jt, jf))| {
                let off = |to: &To| -> u8 {
                    let target = match to {
                        To::Next => return 0,
                        To::Allow => allow,
                        To::Deny => deny,
                        To::NoSys => nosys,
                    };
                    u8::try_from(target - i - 1).expect("filter jump fits")
                };
                Insn {
                    jt: off(jt),
                    jf: off(jf),
                    ..*insn
                }
            })
            .collect();
        out.push(stmt(RET_K, RET_ALLOW));
        out.push(stmt(RET_K, RET_ERRNO | libc::EPERM as u32));
        out.push(stmt(RET_K, RET_ERRNO | libc::ENOSYS as u32));
        Some(out)
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    None
}

/// The program as bubblewrap's `--seccomp` reads it: `struct sock_filter`s
/// back to back, in native byte order.
pub fn bytes() -> Option<Vec<u8>> {
    let prog = program()?;
    let mut out = Vec::with_capacity(prog.len() * 8);
    for i in prog {
        out.extend(i.code.to_ne_bytes());
        out.push(i.jt);
        out.push(i.jf);
        out.extend(i.k.to_ne_bytes());
    }
    Some(out)
}

/// Install `prog` (from [`bytes`]) on the calling thread, for itself and
/// everything it runs. Async-signal-safe: meant for `pre_exec`.
///
/// # Safety
/// Only plain syscalls; `prog` must stay alive for the call.
pub unsafe fn install(prog: &[u8]) -> bool {
    #[repr(C)]
    struct SockFprog {
        len: libc::c_ushort,
        filter: *const u8,
    }
    let fprog = SockFprog {
        len: (prog.len() / 8) as libc::c_ushort,
        filter: prog.as_ptr(),
    };
    // SAFETY: plain syscalls on stack values that outlive them.
    unsafe {
        libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) == 0
            && libc::syscall(
                libc::SYS_seccomp,
                1, // SECCOMP_SET_MODE_FILTER
                0,
                &fprog as *const SockFprog,
            ) == 0
    }
}
