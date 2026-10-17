// SPDX-License-Identifier: EPL-2.0

//! The file dialog's file system: the native one, with a shorter list of devices. On
//! Linux the dialog lists every mounted file system by its device, which under WSL is a
//! row of "none", `/dev/sdd` and `drivers` entries that lead nowhere useful; here it is
//! the root, the Windows drives (`/mnt/c` as `C:`) and other drives mounted under
//! `/mnt`, `/media` or `/run/media`.

use eframe::egui;
use egui_file_dialog::{
    Disk, Disks, FileDialogConfig, FileFilter, FileSystem, Filter, Metadata, NativeFileSystem,
    UserDirectories,
};
use std::io;
use std::path::{Path, PathBuf};

pub struct Files;

impl FileSystem for Files {
    fn metadata(&self, path: &Path) -> io::Result<Metadata> {
        NativeFileSystem.metadata(path)
    }

    fn is_dir(&self, path: &Path) -> bool {
        NativeFileSystem.is_dir(path)
    }

    fn is_file(&self, path: &Path) -> bool {
        NativeFileSystem.is_file(path)
    }

    fn read_dir(&self, path: &Path) -> io::Result<Vec<PathBuf>> {
        NativeFileSystem.read_dir(path)
    }

    fn get_disks(&self, canonicalize_paths: bool) -> Disks {
        if !cfg!(target_os = "linux") {
            return NativeFileSystem.get_disks(canonicalize_paths);
        }
        let mounts = std::fs::read_to_string("/proc/mounts").unwrap_or_default();
        let disks = devices(&mounts)
            .into_iter()
            .map(|(name, mount)| Disk::new(Some(&name), &mount, false, canonicalize_paths))
            .collect();
        Disks::new(disks)
    }

    fn is_path_hidden(&self, path: &Path) -> bool {
        NativeFileSystem.is_path_hidden(path)
    }

    fn create_dir(&self, path: &Path) -> io::Result<()> {
        NativeFileSystem.create_dir(path)
    }

    fn user_dirs(&self, canonicalize_paths: bool) -> Option<UserDirectories> {
        NativeFileSystem.user_dirs(canonicalize_paths)
    }

    fn current_dir(&self) -> io::Result<PathBuf> {
        NativeFileSystem.current_dir()
    }

    fn load_text_file_preview(&self, path: &Path, max_chars: usize) -> io::Result<String> {
        NativeFileSystem.load_text_file_preview(path, max_chars)
    }
}

/// The devices worth listing from `/proc/mounts`' text, as (name, mount point): the root
/// as `/`, a Windows drive under WSL (`C:\` mounted on `/mnt/c`) as `C:`, and other
/// drives under `/mnt`, `/media` and `/run/media` by their mount point; each once.
fn devices(mounts: &str) -> Vec<(String, PathBuf)> {
    let mut out: Vec<(String, PathBuf)> = Vec::new();
    for line in mounts.lines() {
        let mut fields = line.split_whitespace();
        let (Some(device), Some(mount)) = (fields.next(), fields.next()) else {
            continue;
        };
        // /proc/mounts writes spaces and backslashes in octal (`\040`, `\134`).
        let (device, mount) = (unescape(device), unescape(mount));
        let wanted = mount == "/"
            || ["/mnt/", "/media/", "/run/media/"]
                .iter()
                .any(|p| mount.starts_with(p))
                // WSL's own mounts.
                && !mount.starts_with("/mnt/wsl");
        if !wanted || out.iter().any(|(_, m)| m == Path::new(&mount)) {
            continue;
        }
        let drive = device
            .strip_suffix('\\')
            .filter(|d| d.len() == 2 && d.ends_with(':'));
        let name = match drive {
            Some(drive) => drive.to_string(),
            None => mount.clone(),
        };
        out.push((name, PathBuf::from(mount)));
    }
    out
}

/// A `/proc/mounts` field with its octal escapes (`\040`) undone.
fn unescape(field: &str) -> String {
    let bytes = field.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let octal = bytes
            .get(i + 1..i + 4)
            .filter(|o| bytes[i] == b'\\' && o.iter().all(|b| (b'0'..=b'7').contains(b)));
        match octal {
            Some(o) => {
                let value = o.iter().fold(0u32, |v, b| v * 8 + u32::from(b - b'0'));
                out.push(value as u8);
                i += 4;
            }
            None => {
                out.push(bytes[i]);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The file type filters the dialog offers (besides all files), the first selected:
/// `(name, extensions)`.
pub fn set_filters(config: &mut FileDialogConfig, filters: &[(&str, &'static [&'static str])]) {
    config.file_filters = filters
        .iter()
        .map(|&(name, extensions)| FileFilter {
            id: egui::Id::new(("file-filter", name)),
            name: name.to_string(),
            filter: Filter::new(move |p: &Path| has_extension(p, extensions)),
        })
        .collect();
    config.default_file_filter = filters.first().map(|(name, _)| name.to_string());
}

/// Whether `path` has one of `extensions`, in any case.
fn has_extension(path: &Path, extensions: &[&str]) -> bool {
    path.extension().is_some_and(|e| {
        extensions
            .iter()
            .any(|x| e.eq_ignore_ascii_case(std::ffi::OsStr::new(x)))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_the_root_and_the_drives() {
        let mounts = "none /mnt/wsl tmpfs rw 0 0\n\
                      drivers /usr/lib/wsl/drivers 9p ro 0 0\n\
                      /dev/sdd / ext4 rw 0 0\n\
                      none /mnt/wslg tmpfs rw 0 0\n\
                      /dev/sdd /mnt/wslg/distro ext4 ro 0 0\n\
                      none /dev devtmpfs rw 0 0\n\
                      C:\\134 /mnt/c 9p rw 0 0\n\
                      D:\\134 /mnt/d 9p rw 0 0\n\
                      /dev/sdd /snap ext4 rw 0 0\n\
                      /dev/sdb1 /media/nick/USB\\040Stick vfat rw 0 0\n\
                      /dev/sdd / ext4 rw 0 0\n";
        let names: Vec<(String, PathBuf)> = devices(mounts);
        assert_eq!(
            names,
            vec![
                ("/".to_string(), PathBuf::from("/")),
                ("C:".to_string(), PathBuf::from("/mnt/c")),
                ("D:".to_string(), PathBuf::from("/mnt/d")),
                (
                    "/media/nick/USB Stick".to_string(),
                    PathBuf::from("/media/nick/USB Stick")
                ),
            ]
        );
    }

    #[test]
    fn filters_go_by_extension_in_any_case() {
        let mut config = FileDialogConfig::default();
        set_filters(
            &mut config,
            &[("Recordings", &["rsr", "rec"]), ("Tracks", &["track"])],
        );
        assert_eq!(config.default_file_filter.as_deref(), Some("Recordings"));
        assert_eq!(config.file_filters.len(), 2);
        assert!(has_extension(Path::new("a/battle.REC"), &["rsr", "rec"]));
        assert!(!has_extension(Path::new("b.track"), &["rsr", "rec"]));
        assert!(!has_extension(Path::new("rec"), &["rec"]));
        set_filters(&mut config, &[]);
        assert!(config.file_filters.is_empty() && config.default_file_filter.is_none());
    }
}
