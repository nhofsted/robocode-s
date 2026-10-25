// SPDX-License-Identifier: EPL-2.0

//! The Linux sandboxes (ADR 0019). `os` is in layers, so that it runs on any kernel since
//! 3.5 without root or user namespaces: no new privileges, a seccomp filter and rlimits
//! everywhere, and Landlock (5.13+) where the kernel has it, at the newest ABI it offers.
//! `bwrap` runs the program under bubblewrap, with the same filter and rlimits.
//!
//! Everything is prepared before the fork: the child (`pre_exec`) only makes system
//! calls on what is already there, as it must not allocate or take locks.

#![allow(unsafe_code)] // system calls, on values and buffers prepared before the fork

use super::{Grants, Sandbox};
use std::fs::File;
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::FileExt;
use std::os::unix::process::CommandExt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{OnceLock, mpsc};

/// The largest file a robot writes (classic's data quota is 200 KB a robot).
const FSIZE: u64 = 64 << 20;
/// The most files a robot host has open: its jars, a listing's jars, the JVM's own.
const NOFILE: u64 = 4096;

/// What the JVM and the C library read on any system, read only; the ones missing are
/// left out. The libraries; the loader's cache; the time zone and the user's name, as the
/// JVM reports them; the process's own details in `/proc` (Landlock keeps other
/// processes' private ones from it), its cgroup limits and the processors; and the
/// random numbers (without `/dev/random`, Java's `SecureRandom` seeds itself from thread
/// timings, for seconds).
const SYSTEM: [&str; 13] = [
    "/usr",
    "/lib",
    "/lib64",
    "/etc/ld.so.cache",
    "/etc/localtime",
    "/etc/timezone",
    "/etc/nsswitch.conf",
    "/etc/passwd",
    "/proc",
    "/sys/fs/cgroup",
    "/sys/devices/system/cpu",
    "/dev/random",
    "/dev/urandom",
];

/// Why `os` can't run here.
pub(super) fn os_unavailable() -> Option<String> {
    seccomp::ARCH
        .is_none()
        .then(|| "the os sandbox is for x86_64 and aarch64".into())
}

/// Why `bwrap` can't run here: not installed, or no namespaces for it.
pub(super) fn bwrap_unavailable() -> Option<String> {
    os_unavailable().or_else(|| bwrap().err().cloned())
}

pub(super) fn report(sandbox: Sandbox) -> String {
    match (sandbox, landlock_abi()) {
        (Sandbox::Bwrap, _) => "bwrap (namespaces, seccomp)".into(),
        (_, Some(abi)) => format!("os (seccomp, Landlock ABI {abi})"),
        (_, None) => "os (seccomp; filesystem not limited: no Landlock)".into(),
    }
}

/// The kernel's Landlock ABI; `None` without Landlock (not built in, or not enabled).
pub(super) fn landlock_abi() -> Option<u32> {
    static ABI: OnceLock<Option<u32>> = OnceLock::new();
    *ABI.get_or_init(landlock::abi)
}

/// `program` in the `os` sandbox; `landlock: false` leaves Landlock out, as on a kernel
/// without it.
pub(super) fn os_command(program: &Path, grants: &Grants, landlock: bool) -> io::Result<Command> {
    let mut filter = seccomp::Filter::new(true);
    let limits = Limits::now();
    let ruleset = match landlock_abi().filter(|_| landlock) {
        Some(abi) => Some(landlock::ruleset(abi, &rules(program, grants))?),
        None => None,
    };
    let mut command = Command::new(program);
    // SAFETY: only system calls, on memory the closure owns.
    unsafe {
        command.pre_exec(move || {
            limits.apply()?;
            check(libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) as i64)?;
            if let Some(fd) = &ruleset {
                check(libc::syscall(
                    libc::SYS_landlock_restrict_self,
                    fd.as_raw_fd(),
                    0,
                ))?;
            }
            filter.apply()
        });
    }
    Ok(command)
}

/// What `program` may use in the `os` sandbox, with Landlock's rights.
fn rules(program: &Path, grants: &Grants) -> Vec<(PathBuf, u64)> {
    use landlock::*;
    let mut rules = vec![(grants.runtime.clone(), READ | EXECUTE)];
    // The kernel opens the dynamic loader to execute the program. A robot could start it
    // on a program of its own, which then runs in this sandbox too.
    if let Some(loader) = interpreter(program) {
        rules.push((loader, READ_FILE | EXECUTE));
    }
    rules.extend(grants.read.iter().map(|p| (p.clone(), READ)));
    rules.push((grants.home.clone(), WRITE));
    rules.extend(SYSTEM.iter().map(|p| (PathBuf::from(p), READ)));
    rules.push(("/dev/null".into(), READ_FILE | WRITE_FILE));
    rules
}

/// The ELF interpreter (dynamic loader) `program` names.
fn interpreter(program: &Path) -> Option<PathBuf> {
    let file = File::open(program).ok()?;
    let read = |at: u64, len: usize| {
        let mut buf = vec![0; len];
        file.read_exact_at(&mut buf, at).ok().map(|_| buf)
    };
    let u16_at = |b: &[u8], i: usize| u16::from_ne_bytes([b[i], b[i + 1]]);
    let u64_at = |b: &[u8], i: usize| u64::from_ne_bytes(b[i..i + 8].try_into().unwrap());
    let header = read(0, 64)?;
    // 64-bit ELF in the machine's byte order.
    let native = if cfg!(target_endian = "little") { 1 } else { 2 };
    if header[..4] != *b"\x7fELF" || header[4] != 2 || header[5] != native {
        return None;
    }
    let (phoff, size, count) = (
        u64_at(&header, 0x20),
        u16_at(&header, 0x36),
        u16_at(&header, 0x38),
    );
    let headers = read(phoff, usize::from(size) * usize::from(count))?;
    headers.chunks(usize::from(size).max(1)).find_map(|h| {
        const PT_INTERP: u32 = 3;
        if h.len() < 40 || u32::from_ne_bytes(h[..4].try_into().unwrap()) != PT_INTERP {
            return None;
        }
        let path = read(u64_at(h, 8), usize::try_from(u64_at(h, 32)).ok()?)?;
        let path = path.split(|&b| b == 0).next()?;
        Some(PathBuf::from(std::str::from_utf8(path).ok()?))
    })
}

/// bubblewrap, found on the PATH and able to make its namespaces.
fn bwrap() -> Result<&'static PathBuf, &'static String> {
    static BWRAP: OnceLock<Result<PathBuf, String>> = OnceLock::new();
    BWRAP
        .get_or_init(|| {
            let path = std::env::var_os("PATH")
                .and_then(|path| {
                    std::env::split_paths(&path)
                        .map(|dir| dir.join("bwrap"))
                        .find(|p| p.is_file())
                })
                .ok_or("bwrap (bubblewrap) isn't installed")?;
            let output = Command::new(&path)
                .args([
                    "--unshare-all",
                    "--die-with-parent",
                    "--ro-bind",
                    "/",
                    "/",
                    "true",
                ])
                .stdin(Stdio::null())
                .output()
                .map_err(|e| format!("{}: {e}", path.display()))?;
            if !output.status.success() {
                return Err(format!(
                    "bwrap can't make its namespaces here: {} (user namespaces may be off, \
                     or restricted, as Ubuntu's AppArmor does for programs it has no \
                     profile for)",
                    String::from_utf8_lossy(&output.stderr).trim()
                ));
            }
            Ok(path)
        })
        .as_ref()
}

/// `program` under bubblewrap: a mount namespace with only the grants and the system's
/// libraries and settings in it (read only, the home writable), no network, its own
/// processes, a new session (no terminal to type into), and the seccomp filter, which
/// bwrap installs just before it starts the program. Landlock can't come on top: a
/// Landlock domain may not mount, and bwrap must.
pub(super) fn bwrap_command(program: &Path, grants: &Grants) -> io::Result<Command> {
    let bwrap = bwrap().map_err(|e| io::Error::new(io::ErrorKind::Unsupported, e.clone()))?;
    let filter = seccomp::Filter::new(false).to_memfd()?;
    let limits = Limits::now();
    let mut command = Command::new(bwrap);
    command.args(["--unshare-all", "--die-with-parent", "--new-session"]);
    for system in SYSTEM.iter().filter(|p| !p.starts_with("/dev/")) {
        let p = Path::new(system);
        match (*system, std::fs::read_link(p)) {
            ("/proc", _) => command.args(["--proc", "/proc"]),
            (_, Ok(target)) => command.arg("--symlink").arg(target).arg(p),
            _ => command.arg("--ro-bind-try").arg(p).arg(p),
        };
    }
    command.args(["--dev", "/dev", "--tmpfs", "/tmp"]);
    for p in std::iter::once(&grants.runtime).chain(&grants.read) {
        command.arg("--ro-bind-try").arg(p).arg(p);
    }
    command
        .arg("--bind")
        .arg(&grants.home)
        .arg(&grants.home)
        .arg("--chdir")
        .arg(&grants.home)
        .arg("--seccomp")
        .arg(filter.as_raw_fd().to_string())
        .arg("--")
        .arg(program);
    // SAFETY: only system calls, on memory the closure owns.
    unsafe {
        command.pre_exec(move || {
            limits.apply()?;
            // bwrap reads the filter from it: open in bwrap only.
            check(libc::fcntl(filter.as_raw_fd(), libc::F_SETFD, 0) as i64)
        });
    }
    Ok(command)
}

/// Has the kernel kill the program when the runner dies (`PR_SET_PDEATHSIG`). The kernel
/// sends it when the *thread* that started the program ends, so the program must be
/// started by [`spawn_from_keeper`]. Done after the sandbox's own steps: the filter allows
/// `prctl`.
pub(super) fn die_with_parent(command: &mut Command) {
    let runner = std::process::id() as libc::pid_t;
    // SAFETY: only system calls.
    unsafe {
        command.pre_exec(move || {
            check(libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL, 0, 0, 0) as i64)?;
            // The runner died before the signal was set up: nothing would send it.
            if libc::getppid() != runner {
                return Err(io::Error::from_raw_os_error(libc::ESRCH));
            }
            Ok(())
        });
    }
}

/// Starts `command` on a thread that lives as long as the process, for
/// [`die_with_parent`] (and bwrap's `--die-with-parent`, the same signal): robot hosts are
/// started by battle threads, which end before them (a warm host outlives the battle that
/// started it).
pub(super) fn spawn_from_keeper(command: Command) -> io::Result<std::process::Child> {
    type Job = (Command, mpsc::SyncSender<io::Result<std::process::Child>>);
    static KEEPER: OnceLock<Option<mpsc::Sender<Job>>> = OnceLock::new();
    let keeper = KEEPER.get_or_init(|| {
        let (jobs, rx) = mpsc::channel::<Job>();
        let started = std::thread::Builder::new()
            .name("robot starter".into())
            .spawn(move || {
                for (mut command, reply) in rx {
                    // A panic would end the thread, and with it every program it started.
                    let spawned = catch_unwind(AssertUnwindSafe(|| command.spawn()))
                        .unwrap_or_else(|_| Err(io::Error::other("starting it panicked")));
                    let _ = reply.send(spawned);
                }
            });
        started.ok().map(|_| jobs)
    });
    let gone = || io::Error::other("the thread that starts robot hosts is gone");
    let keeper = keeper.as_ref().ok_or_else(gone)?;
    let (reply, spawned) = mpsc::sync_channel(1);
    keeper.send((command, reply)).map_err(|_| gone())?;
    spawned.recv().map_err(|_| gone())?
}

/// A system call's result: -1 is its error.
fn check(result: i64) -> io::Result<()> {
    if result == -1 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// The rlimits: no core dumps, files up to [`FSIZE`] and [`NOFILE`] open files, each at
/// most the current hard limit. No limit on processes: `RLIMIT_NPROC` counts all of the
/// user's processes and threads (a JVM has dozens), so it would fail robots at random
/// on a busy machine. No `RLIMIT_AS`: the JVM reserves far more address space than it
/// uses; `-Xmx` bounds the heap.
#[derive(Clone, Copy)]
struct Limits([(Resource, libc::rlimit); 3]);

#[cfg(target_env = "gnu")]
type Resource = libc::__rlimit_resource_t;
#[cfg(not(target_env = "gnu"))]
type Resource = libc::c_int;

impl Limits {
    fn now() -> Self {
        let limit = |resource, most: u64| {
            let mut now = libc::rlimit {
                rlim_cur: 0,
                rlim_max: 0,
            };
            // SAFETY: fills `now`.
            let max = if unsafe { libc::getrlimit(resource, &mut now) } == 0 {
                now.rlim_max.min(most)
            } else {
                most
            };
            (
                resource,
                libc::rlimit {
                    rlim_cur: max,
                    rlim_max: max,
                },
            )
        };
        Self([
            limit(libc::RLIMIT_CORE, 0),
            limit(libc::RLIMIT_FSIZE, FSIZE),
            limit(libc::RLIMIT_NOFILE, NOFILE),
        ])
    }

    fn apply(&self) -> io::Result<()> {
        for (resource, limit) in &self.0 {
            // SAFETY: reads `limit`.
            check(unsafe { libc::setrlimit(*resource, limit) } as i64)?;
        }
        Ok(())
    }
}

/// Landlock's system calls and rights (`linux/landlock.h`).
mod landlock {
    use super::check;
    use std::fs::File;
    use std::io;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::os::unix::fs::OpenOptionsExt;
    use std::path::PathBuf;

    pub const EXECUTE: u64 = 1 << 0;
    pub const WRITE_FILE: u64 = 1 << 1;
    pub const READ_FILE: u64 = 1 << 2;
    const READ_DIR: u64 = 1 << 3;
    const MAKE_CHAR: u64 = 1 << 6;
    const MAKE_BLOCK: u64 = 1 << 11;
    /// ABI 2.
    const REFER: u64 = 1 << 13;
    /// ABI 3.
    const TRUNCATE: u64 = 1 << 14;
    /// ABI 5.
    const IOCTL_DEV: u64 = 1 << 15;
    /// The rights that apply to a file (the others are about directories).
    const FILE: u64 = EXECUTE | WRITE_FILE | READ_FILE | TRUNCATE | IOCTL_DEV;
    pub const READ: u64 = READ_FILE | READ_DIR;
    /// All but executing and making devices.
    pub const WRITE: u64 = !(EXECUTE | MAKE_CHAR | MAKE_BLOCK | IOCTL_DEV);

    /// ABI 4: TCP's bind and connect.
    const NET_TCP: u64 = 0b11;
    /// ABI 6: abstract Unix sockets and signals of processes outside the sandbox.
    const SCOPED: u64 = 0b11;

    #[repr(C)]
    struct RulesetAttr {
        handled_access_fs: u64,
        handled_access_net: u64,
        scoped: u64,
    }

    #[repr(C, packed)]
    struct PathBeneathAttr {
        allowed_access: u64,
        parent_fd: i32,
    }

    pub fn abi() -> Option<u32> {
        const CREATE_RULESET_VERSION: u32 = 1 << 0;
        // SAFETY: no attributes, only the flag.
        let abi = unsafe {
            libc::syscall(
                libc::SYS_landlock_create_ruleset,
                std::ptr::null::<RulesetAttr>(),
                0,
                CREATE_RULESET_VERSION,
            )
        };
        u32::try_from(abi).ok().filter(|&abi| abi > 0)
    }

    /// The file rights this ABI knows.
    fn handled(abi: u32) -> u64 {
        let mut rights = (1 << 13) - 1;
        for (since, right) in [(2, REFER), (3, TRUNCATE), (5, IOCTL_DEV)] {
            if abi >= since {
                rights |= right;
            }
        }
        rights
    }

    /// A ruleset that denies all the kernel can limit but `rules` (paths with their
    /// rights; paths that don't exist are left out): TCP from ABI 4, abstract Unix
    /// sockets and signals outside it from ABI 6.
    pub fn ruleset(abi: u32, rules: &[(PathBuf, u64)]) -> io::Result<OwnedFd> {
        let handled = handled(abi);
        let attr = RulesetAttr {
            handled_access_fs: handled,
            handled_access_net: if abi >= 4 { NET_TCP } else { 0 },
            scoped: if abi >= 6 { SCOPED } else { 0 },
        };
        // SAFETY: reads `attr`; the result is a new file descriptor.
        let fd = unsafe {
            libc::syscall(
                libc::SYS_landlock_create_ruleset,
                &attr,
                size_of::<RulesetAttr>(),
                0,
            )
        };
        check(fd)?;
        // SAFETY: a file descriptor of our own.
        let ruleset = unsafe { OwnedFd::from_raw_fd(fd as i32) };
        for (path, rights) in rules {
            let Ok(dir) = std::fs::metadata(path).map(|m| m.is_dir()) else {
                continue;
            };
            let target = File::options()
                .read(true)
                .custom_flags(libc::O_PATH)
                .open(path)?;
            let attr = PathBeneathAttr {
                allowed_access: rights & handled & if dir { u64::MAX } else { FILE },
                parent_fd: target.as_raw_fd(),
            };
            const RULE_PATH_BENEATH: i32 = 1;
            // SAFETY: reads `attr`.
            check(unsafe {
                libc::syscall(
                    libc::SYS_landlock_add_rule,
                    ruleset.as_raw_fd(),
                    RULE_PATH_BENEATH,
                    &attr,
                    0,
                )
            })
            .map_err(|e| io::Error::new(e.kind(), format!("{}: {e}", path.display())))?;
        }
        Ok(ruleset)
    }
}

/// The seccomp filter. It allows what it doesn't name: the JVM uses a broad set of
/// system calls. Denied with an error, so Java gets an exception rather than a crash:
/// sockets other than Unix stream ones (and those can't connect: they would reach the
/// user's D-Bus or Docker sockets), so no network; debugging other processes, mounts
/// and namespaces, kernel keys, BPF, performance counters, io_uring, modules and other
/// administration; typing into the terminal; and with `os`, signals to other
/// processes. Another architecture's system calls (x32, 32-bit) kill the process.
mod seccomp {
    use super::check;
    use libc::{SECCOMP_RET_ALLOW, SECCOMP_RET_ERRNO, SECCOMP_RET_KILL_PROCESS, sock_filter};
    use std::io;

    #[cfg(target_arch = "x86_64")]
    pub const ARCH: Option<u32> = Some(0xc000_003e);
    #[cfg(target_arch = "aarch64")]
    pub const ARCH: Option<u32> = Some(0xc000_00b7);
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    pub const ARCH: Option<u32> = None;

    /// Offsets in `struct seccomp_data`; an argument's low 32 bits.
    const NR: u32 = 0;
    const AUDIT_ARCH: u32 = 4;
    const fn arg(i: u32) -> u32 {
        16 + 8 * i + if cfg!(target_endian = "big") { 4 } else { 0 }
    }

    const fn errno(e: i32) -> u32 {
        SECCOMP_RET_ERRNO | (e as u32 & libc::SECCOMP_RET_DATA)
    }

    const LD: u16 = (libc::BPF_LD | libc::BPF_W | libc::BPF_ABS) as u16;
    const JEQ: u16 = (libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K) as u16;
    const JSET: u16 = (libc::BPF_JMP | libc::BPF_JSET | libc::BPF_K) as u16;
    #[cfg(target_arch = "x86_64")]
    const JGE: u16 = (libc::BPF_JMP | libc::BPF_JGE | libc::BPF_K) as u16;
    const RET: u16 = (libc::BPF_RET | libc::BPF_K) as u16;

    fn op(code: u16, k: u32, jt: u8, jf: u8) -> sock_filter {
        sock_filter { code, jt, jf, k }
    }
    fn ret(k: u32) -> sock_filter {
        op(RET, k, 0, 0)
    }

    /// Denied with `EPERM`.
    #[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
    const DENIED: &[libc::c_long] = &[
        libc::SYS_ptrace,
        libc::SYS_process_vm_readv,
        libc::SYS_process_vm_writev,
        libc::SYS_kcmp,
        libc::SYS_pidfd_getfd,
        libc::SYS_mount,
        libc::SYS_umount2,
        libc::SYS_pivot_root,
        libc::SYS_chroot,
        libc::SYS_open_tree,
        libc::SYS_move_mount,
        libc::SYS_fsopen,
        libc::SYS_fsconfig,
        libc::SYS_fsmount,
        libc::SYS_fspick,
        libc::SYS_mount_setattr,
        libc::SYS_unshare,
        libc::SYS_setns,
        libc::SYS_bpf,
        libc::SYS_perf_event_open,
        libc::SYS_userfaultfd,
        libc::SYS_io_uring_setup,
        libc::SYS_io_uring_enter,
        libc::SYS_io_uring_register,
        libc::SYS_keyctl,
        libc::SYS_add_key,
        libc::SYS_request_key,
        libc::SYS_kexec_load,
        libc::SYS_kexec_file_load,
        libc::SYS_init_module,
        libc::SYS_finit_module,
        libc::SYS_delete_module,
        libc::SYS_reboot,
        libc::SYS_swapon,
        libc::SYS_swapoff,
        libc::SYS_acct,
        libc::SYS_quotactl,
        libc::SYS_quotactl_fd,
        libc::SYS_syslog,
        libc::SYS_vhangup,
        libc::SYS_fanotify_init,
        libc::SYS_name_to_handle_at,
        libc::SYS_open_by_handle_at,
        libc::SYS_sethostname,
        libc::SYS_setdomainname,
        libc::SYS_settimeofday,
        libc::SYS_clock_settime,
        libc::SYS_connect,
        libc::SYS_tkill,
        libc::SYS_pidfd_send_signal,
        #[cfg(target_arch = "x86_64")]
        libc::SYS_iopl,
        #[cfg(target_arch = "x86_64")]
        libc::SYS_ioperm,
        #[cfg(target_arch = "x86_64")]
        libc::SYS_modify_ldt,
        #[cfg(target_arch = "x86_64")]
        libc::SYS_uselib,
    ];
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    const DENIED: &[libc::c_long] = &[];

    /// A compiled filter. Its rules for signals name the process itself, which only the
    /// child knows: [`Filter::apply`] puts its id in.
    pub struct Filter {
        program: Vec<sock_filter>,
        own_pid: Vec<usize>,
    }

    impl Filter {
        /// With `signals`, signals only go to the process itself (for `os`; `bwrap`
        /// has a process namespace for that).
        pub fn new(signals: bool) -> Self {
            let mut p = vec![
                op(LD, AUDIT_ARCH, 0, 0),
                op(JEQ, ARCH.unwrap_or(0), 1, 0),
                ret(SECCOMP_RET_KILL_PROCESS),
                op(LD, NR, 0, 0),
            ];
            #[cfg(target_arch = "x86_64")]
            p.extend([op(JGE, 0x4000_0000, 0, 1), ret(SECCOMP_RET_KILL_PROCESS)]);
            for &nr in DENIED {
                p.extend([op(JEQ, nr as u32, 0, 1), ret(errno(libc::EPERM))]);
            }
            // glibc falls back to clone, whose flags can be checked.
            p.extend([
                op(JEQ, libc::SYS_clone3 as u32, 0, 1),
                ret(errno(libc::ENOSYS)),
            ]);
            // Each block below checks one system call's arguments and returns; the
            // others jump over it.
            let block = |p: &mut Vec<sock_filter>, nr: libc::c_long, body: &[sock_filter]| {
                p.push(op(JEQ, nr as u32, 0, body.len() as u8));
                p.extend_from_slice(body);
            };
            let unix = libc::AF_UNIX as u32;
            block(
                &mut p,
                libc::SYS_socket,
                &[
                    op(LD, arg(0), 0, 0),
                    op(JEQ, unix, 1, 0),
                    ret(errno(libc::EAFNOSUPPORT)),
                    op(LD, arg(1), 0, 0),
                    // The type without SOCK_NONBLOCK and SOCK_CLOEXEC.
                    op(JSET, 0xf & !(libc::SOCK_STREAM as u32), 0, 1),
                    ret(errno(libc::EACCES)),
                    ret(SECCOMP_RET_ALLOW),
                ],
            );
            block(
                &mut p,
                libc::SYS_socketpair,
                &[
                    op(LD, arg(0), 0, 0),
                    op(JEQ, unix, 0, 1),
                    ret(SECCOMP_RET_ALLOW),
                    ret(errno(libc::EAFNOSUPPORT)),
                ],
            );
            const NAMESPACES: i32 = libc::CLONE_NEWNS
                | libc::CLONE_NEWCGROUP
                | libc::CLONE_NEWUTS
                | libc::CLONE_NEWIPC
                | libc::CLONE_NEWUSER
                | libc::CLONE_NEWPID
                | libc::CLONE_NEWNET;
            block(
                &mut p,
                libc::SYS_clone,
                &[
                    op(LD, arg(0), 0, 0),
                    op(JSET, NAMESPACES as u32, 0, 1),
                    ret(errno(libc::EPERM)),
                    ret(SECCOMP_RET_ALLOW),
                ],
            );
            block(
                &mut p,
                libc::SYS_ioctl,
                &[
                    op(LD, arg(1), 0, 0),
                    op(JEQ, libc::TIOCSTI as u32, 1, 0),
                    op(JEQ, libc::TIOCLINUX as u32, 0, 1),
                    ret(errno(libc::EPERM)),
                    ret(SECCOMP_RET_ALLOW),
                ],
            );
            // Only reading the personality, or the default one.
            block(
                &mut p,
                libc::SYS_personality,
                &[
                    op(LD, arg(0), 0, 0),
                    op(JEQ, 0, 2, 0),
                    op(JEQ, 0xffff_ffff, 1, 0),
                    ret(errno(libc::EPERM)),
                    ret(SECCOMP_RET_ALLOW),
                ],
            );
            let mut own_pid = Vec::new();
            if signals {
                for nr in [
                    libc::SYS_kill,
                    libc::SYS_tgkill,
                    libc::SYS_rt_sigqueueinfo,
                    libc::SYS_rt_tgsigqueueinfo,
                ] {
                    own_pid.push(p.len() + 2);
                    block(
                        &mut p,
                        nr,
                        &[
                            op(LD, arg(0), 0, 0),
                            op(JEQ, 0, 0, 1),
                            ret(SECCOMP_RET_ALLOW),
                            ret(errno(libc::EPERM)),
                        ],
                    );
                }
            }
            p.push(ret(SECCOMP_RET_ALLOW));
            Self {
                program: p,
                own_pid,
            }
        }

        /// Installs the filter on this process, which must have no new privileges.
        /// Called in the child: it allocates nothing.
        pub fn apply(&mut self) -> io::Result<()> {
            // SAFETY: no arguments.
            let pid = unsafe { libc::getpid() } as u32;
            for &i in &self.own_pid {
                self.program[i].k = pid;
            }
            let program = libc::sock_fprog {
                len: self.program.len() as u16,
                filter: self.program.as_mut_ptr(),
            };
            // SAFETY: reads `program`, which points into `self.program`.
            check(unsafe {
                libc::syscall(
                    libc::SYS_seccomp,
                    libc::SECCOMP_SET_MODE_FILTER,
                    0,
                    &program,
                )
            })
        }

        /// The program in a memory file, for bwrap's `--seccomp`.
        pub fn to_memfd(&self) -> io::Result<std::os::fd::OwnedFd> {
            use std::io::{Seek, Write};
            use std::os::fd::FromRawFd;
            // SAFETY: a constant name; the result is a new file descriptor.
            let fd =
                unsafe { libc::memfd_create(c"robocode-s-seccomp".as_ptr(), libc::MFD_CLOEXEC) };
            check(fd as i64)?;
            // SAFETY: a file descriptor of our own.
            let mut file = unsafe { std::fs::File::from_raw_fd(fd) };
            // SAFETY: plain structs, read as their bytes.
            let bytes = unsafe {
                std::slice::from_raw_parts(
                    self.program.as_ptr().cast::<u8>(),
                    self.program.len() * size_of::<sock_filter>(),
                )
            };
            file.write_all(bytes)?;
            file.rewind()?;
            Ok(file.into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::os::fd::{FromRawFd, OwnedFd};
    use std::os::unix::net::{UnixListener, UnixStream};

    /// The probe's folder: `outside` (not granted: a file, a listening Unix socket) and
    /// `home` (its home).
    const PROBE: &str = "ROBOCODES_SANDBOX_PROBE";
    const PARENT: &str = "ROBOCODES_SANDBOX_PARENT";
    const PORT: &str = "ROBOCODES_SANDBOX_PORT";

    /// Not a test of its own: the tests below start this test binary again in a sandbox
    /// to run it, and it reports what it could do: `probe <name> <errno>`, 0 if it could.
    #[test]
    fn probe() {
        let Some(dir) = std::env::var_os(PROBE).map(PathBuf::from) else {
            return;
        };
        let var = |name| std::env::var(name).unwrap().parse::<i32>().unwrap();
        let (parent, port) = (var(PARENT), var(PORT) as u16);
        let call = |r: i64| check(r);
        // SAFETY: plain system calls.
        let socket = |domain, kind| {
            let fd = unsafe { libc::socket(domain, kind, 0) };
            check(fd as i64).map(|_| unsafe { drop(OwnedFd::from_raw_fd(fd)) })
        };
        let core = {
            let mut limit = libc::rlimit {
                rlim_cur: 1,
                rlim_max: 1,
            };
            // SAFETY: fills `limit`.
            unsafe { libc::getrlimit(libc::RLIMIT_CORE, &mut limit) };
            if limit.rlim_max == 0 {
                Ok(())
            } else {
                Err(io::Error::from_raw_os_error(libc::E2BIG))
            }
        };
        let probes: Vec<(&str, io::Result<()>)> = vec![
            ("tcp", socket(libc::AF_INET, libc::SOCK_STREAM)),
            ("udp6", socket(libc::AF_INET6, libc::SOCK_DGRAM)),
            ("unix-dgram", socket(libc::AF_UNIX, libc::SOCK_DGRAM)),
            ("unix-stream", socket(libc::AF_UNIX, libc::SOCK_STREAM)),
            (
                "connect-tcp",
                TcpStream::connect(("127.0.0.1", port)).map(drop),
            ),
            (
                "unix-pair",
                UnixStream::pair().and_then(|(mut a, mut b)| {
                    a.write_all(b"x")?;
                    b.read_exact(&mut [0])
                }),
            ),
            (
                "connect-unix",
                UnixStream::connect(dir.join("outside/socket")).map(drop),
            ),
            (
                "read-outside",
                std::fs::read(dir.join("outside/secret")).map(drop),
            ),
            (
                "write-outside",
                std::fs::write(dir.join("outside/new"), "x"),
            ),
            ("write-home", std::fs::write(dir.join("home/new"), "x")),
            (
                "exec",
                Command::new("/bin/sh")
                    .args(["-c", "exit 0"])
                    .status()
                    .map(drop),
            ),
            (
                "read-parent",
                std::fs::read(format!("/proc/{parent}/environ")).map(drop),
            ),
            // SAFETY: system calls that change nothing here when they are denied.
            (
                "ptrace",
                call(unsafe { libc::ptrace(libc::PTRACE_ATTACH, parent, 0, 0) }),
            ),
            ("signal", call(unsafe { libc::kill(parent, 0) } as i64)),
            (
                "unshare",
                call(unsafe { libc::unshare(libc::CLONE_NEWUSER) } as i64),
            ),
            (
                "clone3",
                call(unsafe { libc::syscall(libc::SYS_clone3, std::ptr::null::<u8>(), 0) }),
            ),
            ("no-core", core),
        ];
        // After libtest's `test ... ` on the same line.
        println!();
        for (name, result) in probes {
            let code = result.err().map_or(0, |e| e.raw_os_error().unwrap_or(-1));
            println!("probe {name} {code}");
        }
    }

    /// Runs [`probe`] in the sandbox `sandbox` makes and returns what it reported.
    fn probe_in(
        sandbox: impl FnOnce(&Path, &Grants) -> io::Result<Command>,
    ) -> HashMap<String, i32> {
        use std::sync::atomic::{AtomicU32, Ordering};
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "robocode-s-sandbox-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("outside")).unwrap();
        std::fs::create_dir_all(dir.join("home")).unwrap();
        std::fs::write(dir.join("outside/secret"), "secret").unwrap();
        let _unix = UnixListener::bind(dir.join("outside/socket")).unwrap();
        let tcp = TcpListener::bind("127.0.0.1:0").unwrap();
        let exe = std::env::current_exe().unwrap();
        let grants = Grants {
            runtime: exe.parent().unwrap().to_path_buf(),
            read: Vec::new(),
            home: dir.join("home"),
            memory_mb: 64,
        };
        let output = sandbox(&exe, &grants)
            .unwrap()
            .args([
                "--exact",
                "sandbox::linux::tests::probe",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(PROBE, &dir)
            .env(PARENT, std::process::id().to_string())
            .env(PORT, tcp.local_addr().unwrap().port().to_string())
            .stdin(Stdio::null())
            .output()
            .unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let got: HashMap<String, i32> = stdout
            .lines()
            .filter_map(|l| {
                let mut words = l.strip_prefix("probe ")?.split(' ');
                Some((words.next()?.to_string(), words.next()?.parse().ok()?))
            })
            .collect();
        assert!(
            got.len() == 17,
            "the probe didn't report: {stdout}\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        got
    }

    /// Checks `got` against `want`: probe name and errno, 0 if it worked, -1 for any
    /// error.
    fn expect(got: &HashMap<String, i32>, want: &[(&str, i32)]) {
        for &(name, errno) in want {
            let code = got[name];
            let ok = match errno {
                -1 => code != 0,
                e => code == e,
            };
            assert!(ok, "{name}: got {code}, expected {errno} (all: {got:?})");
        }
    }

    /// Denied by seccomp and the rlimits, with or without Landlock.
    const EVERYWHERE: [(&str, i32); 11] = [
        ("tcp", libc::EAFNOSUPPORT),
        ("udp6", libc::EAFNOSUPPORT),
        ("connect-tcp", libc::EAFNOSUPPORT),
        ("unix-dgram", libc::EACCES),
        ("unix-stream", 0),
        ("unix-pair", 0),
        ("connect-unix", libc::EPERM),
        ("ptrace", libc::EPERM),
        ("unshare", libc::EPERM),
        ("clone3", libc::ENOSYS),
        ("no-core", 0),
    ];

    #[test]
    fn the_os_sandbox_denies_the_network_and_all_outside_its_grants() {
        if landlock_abi().is_none() {
            eprintln!("skipped: the kernel has no Landlock");
            return;
        }
        let got = probe_in(|p, g| os_command(p, g, true));
        expect(&got, &EVERYWHERE);
        expect(
            &got,
            &[
                ("read-outside", libc::EACCES),
                ("write-outside", libc::EACCES),
                ("write-home", 0),
                ("exec", libc::EACCES),
                ("read-parent", -1),
                ("signal", libc::EPERM),
            ],
        );
    }

    #[test]
    fn without_landlock_the_os_sandbox_still_has_no_network() {
        let got = probe_in(|p, g| os_command(p, g, false));
        expect(&got, &EVERYWHERE);
        // The filesystem isn't limited.
        expect(
            &got,
            &[
                ("read-outside", 0),
                ("write-home", 0),
                ("signal", libc::EPERM),
            ],
        );
    }

    #[test]
    fn bwrap_shows_the_robot_only_its_grants() {
        if let Some(why) = bwrap_unavailable() {
            eprintln!("skipped: {why}");
            return;
        }
        let got = probe_in(bwrap_command);
        expect(&got, &EVERYWHERE);
        expect(
            &got,
            &[
                ("read-outside", libc::ENOENT),
                ("write-outside", libc::ENOENT),
                ("write-home", 0),
                ("read-parent", libc::ENOENT),
                ("signal", -1),
            ],
        );
    }

    #[test]
    fn finds_the_dynamic_loader() {
        let exe = std::env::current_exe().unwrap();
        let loader = interpreter(&exe).expect("a dynamically linked test binary");
        assert!(loader.is_file(), "{}", loader.display());
        assert_eq!(interpreter(Path::new("/etc/passwd")), None);
    }
}
