// SPDX-License-Identifier: EPL-2.0

//! The Windows sandbox (ADR 0019). `os` starts the program in an AppContainer with no
//! capabilities that open anything up: no network (loopback included) and none of the
//! user's files, opted out of what is granted to all app packages (a "less privileged"
//! AppContainer). ACL entries grant it the runtime, the robot host, the agent and the
//! jars, and its home through a capability of that home's own, so that robots don't
//! reach each other's. A Job Object keeps it to one process and away from the desktop,
//! limits its memory and kills it when the runner lets go of it.
//!
//! `Command` can't start a process in an AppContainer, so the runner does
//! (`CreateProcessW` with the attributes), with the arguments, environment and directory
//! the caller put on a `Command`.

#![allow(unsafe_code)] // Win32 calls, on buffers and handles this module owns

use super::Grants;
use std::ffi::{OsStr, c_void};
use std::fs::File;
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::os::windows::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};
use std::ptr::{null, null_mut};
use std::sync::{Mutex, OnceLock};
use windows_sys::Win32::Foundation::{
    ERROR_ALREADY_EXISTS, HANDLE, HANDLE_FLAG_INHERIT, LocalFree, SetHandleInformation, TRUE,
    WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::Security::Authorization::{
    EXPLICIT_ACCESS_W, GRANT_ACCESS, GetNamedSecurityInfoW, NO_MULTIPLE_TRUSTEE, SE_FILE_OBJECT,
    SetEntriesInAclW, SetNamedSecurityInfoW, TRUSTEE_IS_SID, TRUSTEE_IS_UNKNOWN, TRUSTEE_W,
};
use windows_sys::Win32::Security::Isolation::{
    CreateAppContainerProfile, DeriveAppContainerSidFromAppContainerName,
};
use windows_sys::Win32::Security::{
    ACCESS_ALLOWED_ACE, ACE_HEADER, ACL, ACL_SIZE_INFORMATION, AclSizeInformation,
    CONTAINER_INHERIT_ACE, CopySid, DACL_SECURITY_INFORMATION, DeriveCapabilitySidsFromName,
    EqualSid, FreeSid, GetAce, GetAclInformation, GetLengthSid, INHERIT_ONLY_ACE,
    NO_PROPAGATE_INHERIT_ACE, OBJECT_INHERIT_ACE, PSECURITY_DESCRIPTOR, PSID,
    SECURITY_CAPABILITIES, SID_AND_ATTRIBUTES,
};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_ALL_ACCESS, FILE_EXECUTE, FILE_GENERIC_EXECUTE, FILE_GENERIC_READ,
};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_ACTIVE_PROCESS,
    JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOB_OBJECT_LIMIT_PROCESS_MEMORY, JOBOBJECT_BASIC_UI_RESTRICTIONS,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectBasicUIRestrictions,
    JobObjectExtendedLimitInformation, SetInformationJobObject,
};
use windows_sys::Win32::System::Pipes::CreatePipe;
use windows_sys::Win32::System::SystemServices::{
    ACCESS_ALLOWED_ACE_TYPE, JOB_OBJECT_UILIMIT_ALL, SE_GROUP_ENABLED,
};
use windows_sys::Win32::System::Threading::{
    CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, DETACHED_PROCESS, DeleteProcThreadAttributeList,
    EXTENDED_STARTUPINFO_PRESENT, GetExitCodeProcess, INFINITE, InitializeProcThreadAttributeList,
    PROC_THREAD_ATTRIBUTE_ALL_APPLICATION_PACKAGES_POLICY,
    PROC_THREAD_ATTRIBUTE_CHILD_PROCESS_POLICY, PROC_THREAD_ATTRIBUTE_HANDLE_LIST,
    PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES, PROCESS_INFORMATION, ResumeThread,
    STARTF_USESTDHANDLES, STARTUPINFOEXW, TerminateProcess, UpdateProcThreadAttribute,
    WaitForSingleObject,
};

/// The AppContainer robots run in. Its profile is made once and kept: it holds nothing
/// of the robots' (their homes are granted to them, below the data directory).
const CONTAINER: &str = "robocode-s.robot";

/// The system's capabilities robots get besides their home's, both made for less
/// privileged AppContainers. Reading the system's settings in the registry: without it
/// `jvm.dll` doesn't load ("access denied"), and Winsock doesn't start (with it, sockets
/// are refused). It also opens a few of the user's keys, among them `HKCU\Environment`,
/// the user's own environment variables (a deny entry there doesn't take).
const REGISTRY_READ: &str = "registryRead";
/// The system's random numbers (`CryptGenRandom`): without them Java's `SecureRandom`
/// seeds itself from thread timings, for seconds (as without `/dev/random` on Linux).
const CRYPTO_SERVICES: &str = "lpacCryptoServices";

/// What the JVM commits besides its heap: metaspace, code, threads, the GC's own.
const JVM_OVERHEAD_MB: usize = 512;

/// `winnt.h`: the process may start no processes, and isn't one of "all application
/// packages".
const PROCESS_CREATION_CHILD_PROCESS_RESTRICTED: u32 = 1;
const PROCESS_CREATION_ALL_APPLICATION_PACKAGES_OPT_OUT: u32 = 1;

/// Why `os` can't run here: no AppContainers (before Windows 8), or not allowed.
pub(super) fn os_unavailable() -> Option<String> {
    container().err().cloned()
}

/// `path` without the verbatim prefix `\\?\` that `canonicalize` gives it.
pub(super) fn plain(path: &Path) -> PathBuf {
    match path.to_str() {
        Some(s) if s.starts_with(r"\\?\UNC\") => format!(r"\\{}", &s[8..]).into(),
        Some(s) if s.starts_with(r"\\?\") && s.as_bytes().get(5) == Some(&b':') => s[4..].into(),
        _ => path.to_path_buf(),
    }
}

/// The sandbox's own environment: `PATH` with only the runtime and the system, and the
/// temporary files in the home.
pub(super) fn environment(command: &mut Command, grants: &Grants) {
    let root = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
    let root = PathBuf::from(root);
    let path = std::env::join_paths([
        grants.runtime.join("bin"),
        root.join("System32"),
        root.clone(),
    ]);
    if let Ok(path) = path {
        command.env("PATH", path);
    }
    let tmp = grants.home.join("tmp");
    command.env("TEMP", &tmp).env("TMP", &tmp);
}

/// A SID of our own, copied out of the system's allocation (SIDs are 4-byte aligned).
struct Sid(Vec<u32>);

impl Sid {
    /// SAFETY: `sid` is a valid SID.
    unsafe fn copy(sid: PSID) -> Self {
        // SAFETY: reads the valid `sid` into a buffer of its length.
        unsafe {
            let len = GetLengthSid(sid);
            let mut buf = vec![0u32; (len as usize).div_ceil(4)];
            CopySid(len, buf.as_mut_ptr().cast(), sid);
            Self(buf)
        }
    }

    fn as_psid(&self) -> PSID {
        self.0.as_ptr().cast_mut().cast()
    }
}

/// The robots' AppContainer, made (or found) once.
fn container() -> Result<&'static Sid, &'static String> {
    static SID: OnceLock<Result<Sid, String>> = OnceLock::new();
    SID.get_or_init(|| {
        let name = wide(CONTAINER);
        let about = wide("robocode-s robots");
        let mut sid: PSID = null_mut();
        // SAFETY: strings that outlive the call; `sid` is filled.
        let mut result = unsafe {
            CreateAppContainerProfile(
                name.as_ptr(),
                about.as_ptr(),
                about.as_ptr(),
                null(),
                0,
                &mut sid,
            )
        };
        if result == hresult(ERROR_ALREADY_EXISTS) {
            // SAFETY: as above.
            result = unsafe { DeriveAppContainerSidFromAppContainerName(name.as_ptr(), &mut sid) };
        }
        if result < 0 {
            return Err(format!(
                "can't make the robots' AppContainer {CONTAINER}: {}",
                io::Error::from_raw_os_error(result)
            ));
        }
        // SAFETY: the SID just made, freed as its documentation says.
        unsafe {
            let copy = Sid::copy(sid);
            FreeSid(sid);
            Ok(copy)
        }
    })
    .as_ref()
}

/// The capability a robot's home is granted to: a SID derived from the home's path, so
/// that only the robot started with it reaches the home, and a home used again gets the
/// same one.
fn home_capability(home: &Path) -> io::Result<Sid> {
    capability(&format!("robocode-s.home:{}", home.display()))
}

/// The SID of the capability `name`.
fn capability(name: &str) -> io::Result<Sid> {
    let name = wide(name);
    let (mut groups, mut group_count) = (null_mut::<PSID>(), 0);
    let (mut sids, mut count) = (null_mut::<PSID>(), 0);
    // SAFETY: a string that outlives the call; the arrays are filled.
    check(unsafe {
        DeriveCapabilitySidsFromName(
            name.as_ptr(),
            &mut groups,
            &mut group_count,
            &mut sids,
            &mut count,
        )
    })?;
    // SAFETY: the arrays and SIDs just made, freed as the documentation says.
    unsafe {
        let sid = (count > 0).then(|| Sid::copy(*sids));
        for (array, n) in [(groups, group_count), (sids, count)] {
            for i in 0..n as usize {
                LocalFree(*array.add(i));
            }
            LocalFree(array.cast());
        }
        sid.ok_or_else(|| io::Error::other("no capability SID"))
    }
}

/// What a grant lets the sandbox do.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Access {
    /// Read and execute (the runtime).
    Execute,
    /// Read (the robot host, the agent, the jars).
    Read,
    /// All but executing files: the folder and its folders with all rights (a folder's
    /// "execute" is going through it), the files in them without execute.
    Home,
}

/// The inheritance flags an access entry is compared by.
const INHERITANCE: u32 =
    OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE | INHERIT_ONLY_ACE | NO_PROPAGATE_INHERIT_ACE;

/// The access entries (rights, inheritance) of `access` on a folder (`dir`) or a file.
fn entries(access: Access, dir: bool) -> Vec<(u32, u32)> {
    let all = if dir {
        OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE
    } else {
        0
    };
    match access {
        Access::Execute => vec![(FILE_GENERIC_READ | FILE_GENERIC_EXECUTE, all)],
        Access::Read => vec![(FILE_GENERIC_READ, all)],
        Access::Home if dir => vec![
            (FILE_ALL_ACCESS, CONTAINER_INHERIT_ACE),
            (
                FILE_ALL_ACCESS & !FILE_EXECUTE,
                OBJECT_INHERIT_ACE | INHERIT_ONLY_ACE,
            ),
        ],
        Access::Home => vec![(FILE_ALL_ACCESS & !FILE_EXECUTE, 0)],
    }
}

/// Grants `sid` `access` to `path` (and, a folder, all in it), unless it has it already.
/// The entries are inherited by what the folder holds, and stay. A path that doesn't
/// exist is left out.
fn grant(path: &Path, sid: &Sid, access: Access) -> io::Result<()> {
    let Ok(meta) = std::fs::metadata(path) else {
        return Ok(());
    };
    add_entries(&wide(path), sid, &entries(access, meta.is_dir())).map_err(|e| {
        io::Error::new(
            e.kind(),
            format!(
                "can't grant the sandbox {} (robots' files must be on NTFS): {e}",
                path.display()
            ),
        )
    })
}

/// Entries are added one file at a time: two at once on one file would both add theirs.
static ADDING: Mutex<()> = Mutex::new(());

/// Adds the entries (rights, inheritance) that allow `sid` something to the DACL of the
/// file `name`, but those it has. What a folder holds inherits them.
fn add_entries(name: &[u16], sid: &Sid, entries: &[(u32, u32)]) -> io::Result<()> {
    let _adding = ADDING.lock().unwrap_or_else(|e| e.into_inner());
    let mut dacl: *mut ACL = null_mut();
    let mut descriptor: PSECURITY_DESCRIPTOR = null_mut();
    // SAFETY: a string that outlives the call; `dacl` points into `descriptor`.
    let error = unsafe {
        GetNamedSecurityInfoW(
            name.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            null_mut(),
            null_mut(),
            &mut dacl,
            null_mut(),
            &mut descriptor,
        )
    };
    if error != 0 {
        return Err(io::Error::from_raw_os_error(error as i32));
    }
    let _descriptor = Local(descriptor);
    // No DACL: everyone may do anything, and an entry would be all the DACL there is.
    if dacl.is_null() {
        return Ok(());
    }
    // SAFETY: the DACL just read.
    let have = unsafe { aces(dacl, sid) };
    let missing: Vec<EXPLICIT_ACCESS_W> = entries
        .iter()
        .filter(|&&(rights, flags)| {
            !have
                .iter()
                .any(|&(r, f)| r & rights == rights && f & INHERITANCE == flags)
        })
        .map(|&(rights, flags)| EXPLICIT_ACCESS_W {
            grfAccessPermissions: rights,
            grfAccessMode: GRANT_ACCESS,
            grfInheritance: flags,
            Trustee: TRUSTEE_W {
                pMultipleTrustee: null_mut(),
                MultipleTrusteeOperation: NO_MULTIPLE_TRUSTEE,
                TrusteeForm: TRUSTEE_IS_SID,
                TrusteeType: TRUSTEE_IS_UNKNOWN,
                ptstrName: sid.as_psid().cast(),
            },
        })
        .collect();
    if missing.is_empty() {
        return Ok(());
    }
    let mut new: *mut ACL = null_mut();
    // SAFETY: entries and a DACL that outlive the call; `new` is filled.
    let error = unsafe { SetEntriesInAclW(missing.len() as u32, missing.as_ptr(), dacl, &mut new) };
    if error != 0 {
        return Err(io::Error::from_raw_os_error(error as i32));
    }
    let _new = Local(new.cast());
    // SAFETY: as above. It also gives what a folder holds the inherited entries.
    let error = unsafe {
        SetNamedSecurityInfoW(
            name.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            null_mut(),
            null_mut(),
            new,
            null(),
        )
    };
    if error != 0 {
        return Err(io::Error::from_raw_os_error(error as i32));
    }
    Ok(())
}

/// The rights and flags of the entries in `dacl` that allow `sid` something.
///
/// SAFETY: `dacl` is a valid ACL.
unsafe fn aces(dacl: *const ACL, sid: &Sid) -> Vec<(u32, u32)> {
    let mut info = ACL_SIZE_INFORMATION::default();
    let mut found = Vec::new();
    // SAFETY: reads the valid `dacl`; its entries start with a header.
    unsafe {
        if GetAclInformation(
            dacl,
            (&raw mut info).cast(),
            size_of::<ACL_SIZE_INFORMATION>() as u32,
            AclSizeInformation,
        ) == 0
        {
            return found;
        }
        for i in 0..info.AceCount {
            let mut ace: *mut c_void = null_mut();
            if GetAce(dacl, i, &mut ace) == 0 {
                continue;
            }
            let header = &*ace.cast::<ACE_HEADER>();
            if u32::from(header.AceType) != ACCESS_ALLOWED_ACE_TYPE {
                continue;
            }
            let allowed = ace.cast::<ACCESS_ALLOWED_ACE>();
            if EqualSid((&raw mut (*allowed).SidStart).cast(), sid.as_psid()) != 0 {
                found.push(((*allowed).Mask, u32::from(header.AceFlags)));
            }
        }
    }
    found
}

/// Memory the system allocated, freed with `LocalFree`.
struct Local(*mut c_void);

impl Drop for Local {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: allocated by the system for us.
            unsafe { LocalFree(self.0) };
        }
    }
}

/// A job for one robot: [`JVM_OVERHEAD_MB`] more memory than its heap, one process,
/// killed when the runner closes it (or exits), no unhandled-exception dialog, and none
/// of the desktop: others' windows, the clipboard, the global atoms, the display and
/// system settings, logging off.
fn job(memory_mb: u32) -> io::Result<OwnedHandle> {
    // SAFETY: no name, the default security; the result is a new handle.
    let job = unsafe { CreateJobObjectW(null(), null()) };
    if job.is_null() {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: a handle of our own.
    let job = unsafe { OwnedHandle::from_raw_handle(job) };
    let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
        | JOB_OBJECT_LIMIT_ACTIVE_PROCESS
        | JOB_OBJECT_LIMIT_PROCESS_MEMORY
        | JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION;
    limits.BasicLimitInformation.ActiveProcessLimit = 1;
    limits.ProcessMemoryLimit = (memory_mb as usize + JVM_OVERHEAD_MB) << 20;
    let ui = JOBOBJECT_BASIC_UI_RESTRICTIONS {
        UIRestrictionsClass: JOB_OBJECT_UILIMIT_ALL,
    };
    // SAFETY: the job, with information of the class's size.
    unsafe {
        check(SetInformationJobObject(
            job.as_raw_handle(),
            JobObjectExtendedLimitInformation,
            (&raw const limits).cast(),
            size_of_val(&limits) as u32,
        ))?;
        check(SetInformationJobObject(
            job.as_raw_handle(),
            JobObjectBasicUIRestrictions,
            (&raw const ui).cast(),
            size_of_val(&ui) as u32,
        ))?;
    }
    Ok(job)
}

/// A Win32 `BOOL`: 0 is its error.
fn check(result: i32) -> io::Result<()> {
    if result == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// An HRESULT for a Win32 error.
fn hresult(error: u32) -> i32 {
    (0x8007_0000 | error) as i32
}

/// `s` as a null-terminated wide string.
fn wide(s: impl AsRef<OsStr>) -> Vec<u16> {
    s.as_ref().encode_wide().chain([0]).collect()
}

/// A pipe: our end, and the robot's, which it inherits. `robot_reads`: its stdin.
fn pipe(robot_reads: bool) -> io::Result<(File, OwnedHandle)> {
    let (mut read, mut write): (HANDLE, HANDLE) = (null_mut(), null_mut());
    // SAFETY: the handles are filled.
    check(unsafe { CreatePipe(&mut read, &mut write, null(), 0) })?;
    // SAFETY: handles of our own.
    let (read, write) = unsafe {
        (
            OwnedHandle::from_raw_handle(read),
            OwnedHandle::from_raw_handle(write),
        )
    };
    let (ours, theirs) = if robot_reads {
        (write, read)
    } else {
        (read, write)
    };
    inheritable(&theirs)?;
    Ok((File::from(ours), theirs))
}

/// The null device, for the robot to inherit.
fn null_device() -> io::Result<OwnedHandle> {
    let null = OwnedHandle::from(File::options().read(true).write(true).open("NUL")?);
    inheritable(&null)?;
    Ok(null)
}

fn inheritable(handle: &OwnedHandle) -> io::Result<()> {
    // SAFETY: a handle of our own.
    check(unsafe {
        SetHandleInformation(
            handle.as_raw_handle(),
            HANDLE_FLAG_INHERIT,
            HANDLE_FLAG_INHERIT,
        )
    })
}

/// A process's attribute list.
struct Attributes(Vec<usize>);

impl Attributes {
    fn new(count: u32) -> io::Result<Self> {
        let mut size = 0;
        // SAFETY: asks for the size only (and fails for it).
        unsafe { InitializeProcThreadAttributeList(null_mut(), count, 0, &mut size) };
        let mut list = Self(vec![0; size.div_ceil(size_of::<usize>())]);
        // SAFETY: a buffer of that size.
        check(unsafe { InitializeProcThreadAttributeList(list.as_ptr(), count, 0, &mut size) })?;
        Ok(list)
    }

    fn as_ptr(&mut self) -> *mut c_void {
        self.0.as_mut_ptr().cast()
    }

    /// SAFETY: `value` stays valid until the process is made.
    unsafe fn set<T>(&mut self, attribute: u32, value: &T) -> io::Result<()> {
        // SAFETY: the list, and a value of `T`'s size.
        check(unsafe {
            UpdateProcThreadAttribute(
                self.as_ptr(),
                0,
                attribute as usize,
                (value as *const T).cast(),
                size_of::<T>(),
                null_mut(),
                null(),
            )
        })
    }
}

impl Drop for Attributes {
    fn drop(&mut self) {
        // SAFETY: an initialized list.
        unsafe { DeleteProcThreadAttributeList(self.as_ptr()) };
    }
}

/// `args` as a command line, quoted as the C runtime parses it (Java's launcher does).
fn command_line<'a>(args: impl IntoIterator<Item = &'a OsStr>) -> Vec<u16> {
    let (quote, backslash) = (u16::from(b'"'), u16::from(b'\\'));
    let mut line = Vec::new();
    for arg in args {
        if !line.is_empty() {
            line.push(u16::from(b' '));
        }
        let arg: Vec<u16> = arg.encode_wide().collect();
        let plain = !arg.is_empty()
            && !arg
                .iter()
                .any(|&c| c == quote || c == u16::from(b' ') || c == u16::from(b'\t'));
        if plain {
            line.extend(arg);
            continue;
        }
        line.push(quote);
        let mut backslashes = 0;
        for c in arg {
            if c == backslash {
                backslashes += 1;
            } else {
                if c == quote {
                    // The backslashes before a quote double, and the quote is escaped.
                    line.extend(std::iter::repeat_n(backslash, backslashes + 1));
                }
                backslashes = 0;
            }
            line.push(c);
        }
        // As do those before the closing quote.
        line.extend(std::iter::repeat_n(backslash, backslashes));
        line.push(quote);
    }
    line.push(0);
    line
}

/// The environment `command` sets (it starts from none), as a block for `CreateProcessW`.
fn environment_block(command: &Command) -> Vec<u16> {
    let mut block = Vec::new();
    for (name, value) in command.get_envs() {
        if let Some(value) = value {
            block.extend(name.encode_wide());
            block.push(u16::from(b'='));
            block.extend(value.encode_wide());
            block.push(0);
        }
    }
    if block.is_empty() {
        block.push(0);
    }
    block.push(0);
    block
}

/// A robot's process and its job.
pub(super) struct Process {
    process: OwnedHandle,
    /// Held: closing it kills the process.
    #[allow(dead_code)]
    job: OwnedHandle,
}

/// Starts `command`'s program in the sandbox with `grants`, its stdin and stdout piped
/// (`piped`) or else null, and its stderr piped; returns it with our ends of its pipes.
///
/// The process is detached (no console: a console would be a second process, conhost,
/// which the job and the child process policy refuse), suspended until it is in its job.
/// Its pipe ends are inheritable for the moment it is made, and only they are inherited
/// (the handle list); a plain process another thread starts then may inherit them too,
/// which would only keep a pipe open longer.
pub(super) fn spawn(
    command: &Command,
    grants: &Grants,
    piped: [bool; 2],
) -> io::Result<(Process, [Option<File>; 3])> {
    let container =
        container().map_err(|e| io::Error::new(io::ErrorKind::Unsupported, e.clone()))?;
    let home = home_capability(&grants.home)?;
    let registry = capability(REGISTRY_READ)?;
    let crypto = capability(CRYPTO_SERVICES)?;
    grant(&grants.runtime, container, Access::Execute)?;
    for path in &grants.read {
        grant(path, container, Access::Read)?;
    }
    grant(&grants.home, &home, Access::Home)?;
    let job = job(grants.memory_mb)?;

    let ours_or_null = |piped: bool, robot_reads: bool| -> io::Result<_> {
        if piped {
            let (ours, theirs) = pipe(robot_reads)?;
            Ok((Some(ours), theirs))
        } else {
            Ok((None, null_device()?))
        }
    };
    let (stdin, robot_stdin) = ours_or_null(piped[0], true)?;
    let (stdout, robot_stdout) = ours_or_null(piped[1], false)?;
    let (stderr, robot_stderr) = pipe(false)?;
    let handles: [HANDLE; 3] = [
        robot_stdin.as_raw_handle(),
        robot_stdout.as_raw_handle(),
        robot_stderr.as_raw_handle(),
    ];

    let mut capabilities = [&home, &registry, &crypto].map(|sid| SID_AND_ATTRIBUTES {
        Sid: sid.as_psid(),
        Attributes: SE_GROUP_ENABLED as u32,
    });
    let security = SECURITY_CAPABILITIES {
        AppContainerSid: container.as_psid(),
        Capabilities: capabilities.as_mut_ptr(),
        CapabilityCount: capabilities.len() as u32,
        Reserved: 0,
    };
    let children = PROCESS_CREATION_CHILD_PROCESS_RESTRICTED;
    let packages = PROCESS_CREATION_ALL_APPLICATION_PACKAGES_OPT_OUT;
    let mut attributes = Attributes::new(4)?;
    // SAFETY: values that outlive `CreateProcessW` below.
    unsafe {
        attributes.set(PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES, &security)?;
        attributes.set(PROC_THREAD_ATTRIBUTE_HANDLE_LIST, &handles)?;
        attributes.set(PROC_THREAD_ATTRIBUTE_CHILD_PROCESS_POLICY, &children)?;
        attributes.set(
            PROC_THREAD_ATTRIBUTE_ALL_APPLICATION_PACKAGES_POLICY,
            &packages,
        )?;
    }
    let mut startup = STARTUPINFOEXW::default();
    startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
    startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    [
        startup.StartupInfo.hStdInput,
        startup.StartupInfo.hStdOutput,
        startup.StartupInfo.hStdError,
    ] = handles;
    startup.lpAttributeList = attributes.as_ptr();

    let program = wide(command.get_program());
    let mut line = command_line(std::iter::once(command.get_program()).chain(command.get_args()));
    let environment = environment_block(command);
    let dir = command.get_current_dir().map(wide);
    let mut info = PROCESS_INFORMATION::default();
    // SAFETY: strings, attributes and handles that outlive the call; `info` is filled.
    check(unsafe {
        windows_sys::Win32::System::Threading::CreateProcessW(
            program.as_ptr(),
            line.as_mut_ptr(),
            null(),
            null(),
            TRUE,
            EXTENDED_STARTUPINFO_PRESENT
                | CREATE_SUSPENDED
                | CREATE_UNICODE_ENVIRONMENT
                | DETACHED_PROCESS,
            environment.as_ptr().cast(),
            dir.as_ref().map_or(null(), |d| d.as_ptr()),
            &startup.StartupInfo,
            &mut info,
        )
    })?;
    // SAFETY: the handles just made.
    let (process, thread) = unsafe {
        (
            OwnedHandle::from_raw_handle(info.hProcess),
            OwnedHandle::from_raw_handle(info.hThread),
        )
    };
    let process = Process { process, job };
    // SAFETY: handles of our own.
    let started = unsafe {
        check(AssignProcessToJobObject(
            process.job.as_raw_handle(),
            process.process.as_raw_handle(),
        ))
        .and_then(|()| {
            if ResumeThread(thread.as_raw_handle()) == u32::MAX {
                Err(io::Error::last_os_error())
            } else {
                Ok(())
            }
        })
    };
    if let Err(e) = started {
        // SAFETY: our process.
        unsafe { TerminateProcess(process.process.as_raw_handle(), 1) };
        return Err(e);
    }
    Ok((process, [stdin, stdout, Some(stderr)]))
}

impl Process {
    pub fn wait(&mut self) -> io::Result<ExitStatus> {
        // SAFETY: our process.
        if unsafe { WaitForSingleObject(self.process.as_raw_handle(), INFINITE) } != WAIT_OBJECT_0 {
            return Err(io::Error::last_os_error());
        }
        self.exit_status()
    }

    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        // SAFETY: our process.
        match unsafe { WaitForSingleObject(self.process.as_raw_handle(), 0) } {
            WAIT_OBJECT_0 => self.exit_status().map(Some),
            WAIT_TIMEOUT => Ok(None),
            _ => Err(io::Error::last_os_error()),
        }
    }

    pub fn kill(&mut self) -> io::Result<()> {
        // SAFETY: our process.
        if unsafe { TerminateProcess(self.process.as_raw_handle(), 1) } == 0 {
            let error = io::Error::last_os_error();
            // It had ended already.
            if !matches!(self.try_wait(), Ok(Some(_))) {
                return Err(error);
            }
        }
        Ok(())
    }

    fn exit_status(&self) -> io::Result<ExitStatus> {
        let mut code = 0;
        // SAFETY: our process; `code` is filled.
        check(unsafe { GetExitCodeProcess(self.process.as_raw_handle(), &mut code) })?;
        Ok(ExitStatus::from_raw(code))
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Sandbox, Sandboxed, command};
    use super::*;
    use std::collections::HashMap;
    use std::io::{BufRead, BufReader};
    use std::net::{TcpListener, TcpStream, UdpSocket};
    use std::time::Duration;

    /// The probe's folder: `outside` (not granted: a file), `home` (its home) and `bin`
    /// (the probe itself).
    const PROBE: &str = "ROBOCODES_SANDBOX_PROBE";
    const PARENT: &str = "ROBOCODES_SANDBOX_PARENT";
    /// A TCP listener's and a UDP socket's port.
    const PORT: &str = "ROBOCODES_SANDBOX_PORT";
    const PROFILE: &str = "ROBOCODES_SANDBOX_PROFILE";
    /// Set: the probe only waits, to be killed.
    const WAIT: &str = "ROBOCODES_SANDBOX_WAIT";

    /// The probe's "heap": its job lets it commit [`JVM_OVERHEAD_MB`] more.
    const PROBE_MEMORY_MB: u32 = 64;

    const ERROR_ACCESS_DENIED: i32 = 5;
    const ERROR_NOT_ENOUGH_MEMORY: i32 = 8;
    const ERROR_CHILD_PROCESS_BLOCKED: i32 = 367;
    const WSAEACCES: i32 = 10013;

    /// The error of a Win32 call that returned `ok`.
    fn win32(ok: bool) -> io::Result<()> {
        if ok {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    /// Commits `mb` megabytes.
    fn commit(mb: usize) -> io::Result<()> {
        Vec::<u8>::new()
            .try_reserve_exact(mb << 20)
            .map_err(|_| io::Error::from_raw_os_error(ERROR_NOT_ENOUGH_MEMORY))
    }

    /// Opens the user's registry key `name` to read it.
    fn user_key(name: &str) -> io::Result<()> {
        use windows_sys::Win32::System::Registry::{
            HKEY, HKEY_CURRENT_USER, KEY_READ, RegCloseKey, RegOpenKeyExW,
        };
        let mut key: HKEY = null_mut();
        let name = wide(name);
        // SAFETY: a string that outlives the call; a key it opens is closed.
        match unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, name.as_ptr(), 0, KEY_READ, &mut key) } {
            0 => {
                // SAFETY: as above.
                unsafe { RegCloseKey(key) };
                Ok(())
            }
            e => Err(io::Error::from_raw_os_error(e as i32)),
        }
    }

    /// Opens the process `pid` to read its memory.
    fn open_process(pid: u32) -> io::Result<()> {
        use windows_sys::Win32::System::Threading::{
            OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_VM_READ,
        };
        // SAFETY: a handle it gives is closed.
        let process = unsafe { OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, 0, pid) };
        win32(!process.is_null())?;
        // SAFETY: as above.
        drop(unsafe { OwnedHandle::from_raw_handle(process) });
        Ok(())
    }

    /// Opens the clipboard (and closes it).
    fn clipboard() -> io::Result<()> {
        use windows_sys::Win32::System::DataExchange::{CloseClipboard, OpenClipboard};
        // SAFETY: no window; a clipboard it opens is closed.
        win32(unsafe { OpenClipboard(null_mut()) != 0 && CloseClipboard() != 0 })
    }

    /// Not a test of its own: the tests below start this test binary again in the
    /// sandbox to run it, and it reports what it could do: `probe <name> <error>`, 0 if
    /// it could.
    #[test]
    fn probe() {
        let Some(dir) = std::env::var_os(PROBE).map(PathBuf::from) else {
            return;
        };
        if std::env::var_os(WAIT).is_some() {
            println!("\nprobe waiting");
            loop {
                std::thread::sleep(Duration::from_secs(1));
            }
        }
        let var = |name| std::env::var(name).unwrap();
        let parent: u32 = var(PARENT).parse().unwrap();
        let port: u16 = var(PORT).parse().unwrap();
        let wait = Duration::from_secs(2);
        let probes: Vec<(&str, io::Result<()>)> = vec![
            (
                "connect-loopback",
                TcpStream::connect_timeout(&([127, 0, 0, 1], port).into(), wait).map(drop),
            ),
            (
                "connect-outside",
                TcpStream::connect_timeout(&([1, 1, 1, 1], 443).into(), wait).map(drop),
            ),
            (
                "listen",
                TcpListener::bind("127.0.0.1:0")
                    .and_then(|l| TcpStream::connect_timeout(&l.local_addr()?, wait).map(drop)),
            ),
            (
                "udp",
                UdpSocket::bind("127.0.0.1:0")
                    .and_then(|s| s.send_to(b"x", ("127.0.0.1", port)))
                    .map(drop),
            ),
            ("read-profile", std::fs::read_dir(var(PROFILE)).map(drop)),
            (
                "read-outside",
                std::fs::read(dir.join("outside/secret")).map(drop),
            ),
            (
                "write-outside",
                std::fs::write(dir.join("outside/new"), "x"),
            ),
            (
                "write-home",
                std::fs::write(dir.join("home/new"), "x")
                    .and_then(|()| std::fs::read(dir.join("home/new")).map(drop)),
            ),
            (
                "write-temp",
                std::fs::write(std::env::temp_dir().join("new"), "x"),
            ),
            (
                "exec",
                Command::new(PathBuf::from(var("SystemRoot")).join(r"System32\whoami.exe"))
                    .status()
                    .map(drop),
            ),
            (
                "exec-self",
                Command::new(std::env::current_exe().unwrap())
                    .arg("--list")
                    .status()
                    .map(drop),
            ),
            ("open-parent", open_process(parent)),
            ("registry-environment", user_key("Environment")),
            (
                "registry-run",
                user_key(r"Software\Microsoft\Windows\CurrentVersion\Run"),
            ),
            ("clipboard", clipboard()),
            ("commit-heap", commit(PROBE_MEMORY_MB as usize)),
            (
                "commit-over",
                commit(PROBE_MEMORY_MB as usize + JVM_OVERHEAD_MB),
            ),
        ];
        // After libtest's `test ... ` on the same line.
        println!();
        for (name, result) in probes {
            let code = result.err().map_or(0, |e| e.raw_os_error().unwrap_or(-1));
            println!("probe {name} {code}");
        }
    }

    /// The probe's folder, with `bin/probe.exe`, and its grants.
    fn probe_dir() -> (PathBuf, Grants) {
        use std::sync::atomic::{AtomicU32, Ordering};
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "robocode-s-sandbox-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        for sub in ["outside", "home", "bin"] {
            std::fs::create_dir_all(dir.join(sub)).unwrap();
        }
        std::fs::write(dir.join("outside/secret"), "secret").unwrap();
        // A copy, so that the grant is on a folder of the test's own.
        std::fs::copy(std::env::current_exe().unwrap(), dir.join("bin/probe.exe")).unwrap();
        let grants = Grants {
            runtime: dir.join("bin"),
            read: Vec::new(),
            home: dir.join("home"),
            memory_mb: PROBE_MEMORY_MB,
        };
        (dir, grants)
    }

    /// The command that runs [`probe`] in the sandbox.
    fn probe_command(dir: &Path, grants: &Grants, port: u16) -> Sandboxed {
        let mut sandboxed = command(Sandbox::Os, &dir.join("bin/probe.exe"), grants).unwrap();
        sandboxed
            .command
            .args([
                "--exact",
                "sandbox::windows::tests::probe",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(PROBE, dir)
            .env(PARENT, std::process::id().to_string())
            .env(PORT, port.to_string())
            .env(PROFILE, std::env::var_os("USERPROFILE").unwrap())
            .current_dir(&grants.home);
        sandboxed
    }

    /// Runs [`probe`] in the sandbox and returns what it reported, and `udp-received`:
    /// 0 if what it sent by UDP came.
    fn probe_in_sandbox() -> HashMap<String, i32> {
        let (dir, grants) = probe_dir();
        let tcp = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = tcp.local_addr().unwrap().port();
        let udp = UdpSocket::bind(("127.0.0.1", port)).unwrap();
        udp.set_nonblocking(true).unwrap();
        let output = probe_command(&dir, &grants, port).output().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut got: HashMap<String, i32> = stdout
            .lines()
            .filter_map(|l| {
                let mut words = l.strip_prefix("probe ")?.split(' ');
                Some((words.next()?.to_string(), words.next()?.parse().ok()?))
            })
            .collect();
        assert!(
            got.len() == 17,
            "the probe didn't report ({}): {stdout}\n{}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        got.insert(
            "udp-received".into(),
            udp.recv(&mut [0; 8]).map_or(1, |_| 0),
        );
        got
    }

    #[test]
    fn the_os_sandbox_denies_the_network_and_all_outside_its_grants() {
        let got = probe_in_sandbox();
        for (name, want) in [
            ("connect-loopback", WSAEACCES),
            ("connect-outside", WSAEACCES),
            ("listen", WSAEACCES),
            ("udp", WSAEACCES),
            ("udp-received", 1),
            ("read-profile", ERROR_ACCESS_DENIED),
            ("read-outside", ERROR_ACCESS_DENIED),
            ("write-outside", ERROR_ACCESS_DENIED),
            ("write-home", 0),
            ("write-temp", 0),
            ("exec", ERROR_CHILD_PROCESS_BLOCKED),
            ("exec-self", ERROR_CHILD_PROCESS_BLOCKED),
            ("open-parent", ERROR_ACCESS_DENIED),
            // `registryRead` opens it (see [`REGISTRY_READ`]).
            ("registry-environment", 0),
            ("registry-run", ERROR_ACCESS_DENIED),
            ("clipboard", ERROR_ACCESS_DENIED),
            ("commit-heap", 0),
            ("commit-over", ERROR_NOT_ENOUGH_MEMORY),
        ] {
            let code = got[name];
            assert!(
                code == want,
                "{name}: got {code}, expected {want} (all: {got:?})"
            );
        }
    }

    #[test]
    fn the_job_kills_the_robot_when_the_runner_lets_go() {
        let (dir, grants) = probe_dir();
        let mut sandboxed = probe_command(&dir, &grants, 0);
        sandboxed.command.env(WAIT, "1");
        let (process, [_, stdout, _]) = spawn(&sandboxed.command, &grants, [false, true]).unwrap();
        let waiting = BufReader::new(stdout.unwrap())
            .lines()
            .map_while(Result::ok)
            .any(|l| l == "probe waiting");
        assert!(waiting);
        let Process { process, job } = process;
        // SAFETY: our process.
        let running = unsafe { WaitForSingleObject(process.as_raw_handle(), 0) };
        assert_eq!(running, WAIT_TIMEOUT);
        drop(job);
        // SAFETY: as above.
        let ended = unsafe { WaitForSingleObject(process.as_raw_handle(), 10_000) };
        assert_eq!(ended, WAIT_OBJECT_0, "the robot outlived its job");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The entries in `name`'s DACL that allow `sid` something: (rights, flags).
    fn entries_for(name: &Path, sid: &Sid) -> Vec<(u32, u32)> {
        let name = wide(name);
        let (mut dacl, mut descriptor) = (null_mut::<ACL>(), null_mut());
        // SAFETY: as in `add_entries`.
        unsafe {
            let error = GetNamedSecurityInfoW(
                name.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                null_mut(),
                null_mut(),
                &mut dacl,
                null_mut(),
                &mut descriptor,
            );
            assert_eq!(error, 0);
            let _descriptor = Local(descriptor);
            aces(dacl, sid)
        }
    }

    #[test]
    fn grants_are_added_once_and_inherited() {
        const INHERITED_ACE: u32 = 0x10;
        let (dir, grants) = probe_dir();
        let sid = container().unwrap();
        let home = home_capability(&grants.home).unwrap();
        assert!(home.0 != home_capability(&dir.join("elsewhere")).unwrap().0);
        assert!(home.0 == home_capability(&grants.home).unwrap().0);
        let secret = dir.join("outside/secret");
        for _ in 0..2 {
            grant(&grants.runtime, sid, Access::Execute).unwrap();
            grant(&secret, sid, Access::Read).unwrap();
            grant(&grants.home, &home, Access::Home).unwrap();
        }
        let own = |path: &Path, sid: &Sid| {
            entries_for(path, sid)
                .into_iter()
                .filter(|&(_, flags)| flags & INHERITED_ACE == 0)
                .count()
        };
        assert_eq!(own(&grants.runtime, sid), 1);
        assert_eq!(own(&secret, sid), 1);
        assert_eq!(own(&grants.home, &home), 2);
        assert_eq!(own(&grants.home, sid), 0);
        let exe = entries_for(&grants.runtime.join("probe.exe"), sid);
        assert!(
            exe.iter().any(|&(r, f)| f & INHERITED_ACE != 0
                && r & FILE_GENERIC_EXECUTE == FILE_GENERIC_EXECUTE),
            "{exe:?}"
        );
        std::fs::write(grants.home.join("data"), "x").unwrap();
        let data = entries_for(&grants.home.join("data"), &home);
        assert!(
            !data.is_empty() && data.iter().all(|&(r, _)| r & FILE_EXECUTE == 0),
            "{data:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn arguments_are_quoted_as_the_c_runtime_reads_them() {
        let line =
            command_line(["java", "a b", "", r"C:\x y\", r#"say "hi""#, r"a\\b"].map(OsStr::new));
        let line = String::from_utf16(&line[..line.len() - 1]).unwrap();
        assert_eq!(line, r#"java "a b" "" "C:\x y\\" "say \"hi\"" a\\b"#);
        assert_eq!(
            plain(Path::new(r"\\?\C:\robocode\jre")),
            Path::new(r"C:\robocode\jre")
        );
        assert_eq!(
            plain(Path::new(r"\\?\UNC\server\share")),
            Path::new(r"\\server\share")
        );
    }
}
