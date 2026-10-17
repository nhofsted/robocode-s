// SPDX-License-Identifier: EPL-2.0

//! Gives the Windows program its icon (`assets/robocode-s.ico`, see `assets/README.md`)
//! and version information, as resources. Only when building for Windows, cross builds
//! included (the target's OS, not the host's); other builds need no resource compiler.

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rerun-if-changed=assets/robocode-s.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut res = winresource::WindowsResource::new();
    res.set_icon("assets/robocode-s.ico")
        .set("ProductName", "robocode-s")
        .set("FileDescription", "robocode-s");
    if let Err(e) = res.compile() {
        panic!("could not compile the Windows resources (windres, from MinGW-w64): {e}");
    }
}
