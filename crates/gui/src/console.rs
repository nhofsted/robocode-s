// SPDX-License-Identifier: EPL-2.0

//! The terminal on Windows. A release build there is a GUI program, so that starting it
//! opens no console window; it has no stdout or stderr then, and `--help`, errors and
//! the `ROBOCODES_*_LOG` diagnostics would go nowhere. Started from a terminal, it
//! writes to that terminal's console instead (which doesn't wait for it: its prompt
//! comes back at once, and the output lands after it).
#![allow(unsafe_code)] // Win32 calls, with plain values and a null-terminated name

use windows_sys::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows_sys::Win32::System::Console::{
    ATTACH_PARENT_PROCESS, AttachConsole, GetStdHandle, STD_ERROR_HANDLE, STD_HANDLE,
    STD_OUTPUT_HANDLE, SetStdHandle,
};

/// Whether the process has no `id` stream (std looks it up at every write).
fn unset(id: STD_HANDLE) -> bool {
    // SAFETY: takes a constant, returns a handle or null.
    let handle = unsafe { GetStdHandle(id) };
    handle.is_null() || handle == INVALID_HANDLE_VALUE
}

/// Points stdout and stderr, where they go nowhere, at the console of the process that
/// started this one, if it has one. Redirected streams (a pipe, a file) are left alone.
/// Call it first.
pub fn attach() {
    let streams = [STD_OUTPUT_HANDLE, STD_ERROR_HANDLE];
    if !streams.into_iter().any(unset) {
        return;
    }
    // SAFETY: takes a constant; fails when there is no parent console, or one already.
    if unsafe { AttachConsole(ATTACH_PARENT_PROCESS) } == 0 {
        return;
    }
    let name: Vec<u16> = "CONOUT$\0".encode_utf16().collect();
    for id in streams.into_iter().filter(|&id| unset(id)) {
        // SAFETY: `name` is null-terminated and outlives the call; the other pointers
        // are null, as allowed.
        let handle = unsafe {
            CreateFileW(
                name.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                std::ptr::null(),
                OPEN_EXISTING,
                0,
                std::ptr::null_mut(),
            )
        };
        if handle != INVALID_HANDLE_VALUE {
            // SAFETY: a handle just opened, kept open for the process's life.
            unsafe { SetStdHandle(id, handle) };
        }
    }
}
