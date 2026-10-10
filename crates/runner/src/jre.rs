// SPDX-License-Identifier: EPL-2.0

//! The Java runtime for Java robots: a pinned Eclipse Temurin JRE, downloaded on first
//! use and checked against its SHA-256 (design §6.1, O5).

use sha2::{Digest, Sha256};
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

/// The pinned release.
pub const RELEASE: &str = "jdk-21.0.12.1+1";

/// One downloadable JRE package.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Package {
    pub file: &'static str,
    pub sha256: &'static str,
}

/// The package for the current platform, if Temurin provides one.
pub fn package() -> Option<Package> {
    let (file, sha256) = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => (
            "OpenJDK21U-jre_x64_linux_hotspot_21.0.12.1_1.tar.gz",
            "2413149700df0f7d440500a84a8f764c535f21e5a5e87d38328b64eec2c5b500",
        ),
        ("linux", "aarch64") => (
            "OpenJDK21U-jre_aarch64_linux_hotspot_21.0.12.1_1.tar.gz",
            "14be1f35ebdbd1f6e8d57eb911a3ffb74d6d9aa255abc5daf2b1302002cf2cf2",
        ),
        ("macos", "x86_64") => (
            "OpenJDK21U-jre_x64_mac_hotspot_21.0.12.1_1.tar.gz",
            "6717ec641fd9ce0bb209ca083ee23b42202ac68cb6fcc5753496e0e4a0f41989",
        ),
        ("macos", "aarch64") => (
            "OpenJDK21U-jre_aarch64_mac_hotspot_21.0.12.1_1.tar.gz",
            "dec50fc6f9fcd4fe3ae8cabf5a5fa68f6afc48841f7698e468e9aa5d54beed84",
        ),
        ("windows", "x86_64") => (
            "OpenJDK21U-jre_x64_windows_hotspot_21.0.12.1_1.zip",
            "d35f31e712f0fcf6ac5a093edc90204fbff22f720ba3950bd09d331d5e621636",
        ),
        _ => return None,
    };
    Some(Package { file, sha256 })
}

fn url(package: &Package) -> String {
    format!(
        "https://github.com/adoptium/temurin21-binaries/releases/download/{}/{}",
        RELEASE.replace('+', "%2B"),
        package.file
    )
}

/// The `java` executable to run robots with: `$ROBOCODES_JAVA` if set, else the pinned
/// JRE in `data_dir`, downloaded first if needed.
pub fn java(data_dir: &Path) -> io::Result<PathBuf> {
    if let Some(java) = std::env::var_os("ROBOCODES_JAVA") {
        return Ok(PathBuf::from(java));
    }
    let package = package().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::Unsupported,
            "no pinned JRE for this platform; set ROBOCODES_JAVA",
        )
    })?;
    let dir = data_dir.join("jre").join(RELEASE);
    if let Some(java) = find_java(&dir) {
        return Ok(java);
    }
    let bytes = download(&url(&package))?;
    verify(&bytes, package.sha256)?;
    let staging = data_dir.join("jre").join(format!("{RELEASE}.partial"));
    let _ = fs::remove_dir_all(&staging);
    fs::create_dir_all(&staging)?;
    if package.file.ends_with(".zip") {
        zip::ZipArchive::new(io::Cursor::new(bytes))
            .and_then(|mut z| z.extract(&staging))
            .map_err(io::Error::other)?;
    } else {
        tar::Archive::new(flate2::read::GzDecoder::new(bytes.as_slice())).unpack(&staging)?;
    }
    fs::rename(&staging, &dir)?;
    find_java(&dir)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no java in the downloaded JRE"))
}

fn download(url: &str) -> io::Result<Vec<u8>> {
    let mut response = ureq::get(url).call().map_err(io::Error::other)?;
    let mut bytes = Vec::new();
    response
        .body_mut()
        .with_config()
        .limit(512 * 1024 * 1024)
        .reader()
        .read_to_end(&mut bytes)?;
    Ok(bytes)
}

/// Fails unless the bytes have the expected SHA-256 (hex).
pub fn verify(bytes: &[u8], sha256: &str) -> io::Result<()> {
    let actual: String = Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    if actual == sha256 {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("checksum mismatch: expected {sha256}, got {actual}"),
        ))
    }
}

/// The `java` executable inside an unpacked JRE (`<top>/bin/java` or, on macOS,
/// `<top>/Contents/Home/bin/java`).
fn find_java(dir: &Path) -> Option<PathBuf> {
    let exe = if cfg!(windows) { "java.exe" } else { "java" };
    for top in fs::read_dir(dir).ok()?.flatten() {
        for candidate in [
            top.path().join("bin").join(exe),
            top.path().join("Contents/Home/bin").join(exe),
        ] {
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verify_checks_sha256() {
        // SHA-256 of "abc".
        let abc = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        assert!(verify(b"abc", abc).is_ok());
        assert!(verify(b"abd", abc).is_err());
    }

    #[test]
    fn this_platform_has_a_pinned_package() {
        let p = package().expect("CI platforms have a pinned JRE");
        assert_eq!(p.sha256.len(), 64);
        assert!(url(&p).contains("jdk-21.0.12.1%2B1"));
    }

    /// Downloads the real JRE; only with ROBOCODES_TEST_DOWNLOAD set (about 50 MB).
    #[test]
    fn downloads_and_unpacks_the_jre() {
        if std::env::var_os("ROBOCODES_TEST_DOWNLOAD").is_none() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("robocodes-jre-test-{}", std::process::id()));
        let java = java(&dir).unwrap();
        let output = std::process::Command::new(&java)
            .arg("-version")
            .output()
            .unwrap();
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("21.0.12"),
            "{output:?}"
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
