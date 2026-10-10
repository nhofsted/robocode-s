// SPDX-License-Identifier: EPL-2.0

//! `robocode-s` command-line interface.

fn main() {
    match std::env::args().nth(1).as_deref() {
        Some("--version" | "-V") => println!("robocode-s {}", env!("CARGO_PKG_VERSION")),
        _ => {
            eprintln!(
                "robocode-s {}: nothing to run yet",
                env!("CARGO_PKG_VERSION")
            );
            eprintln!("usage: robocode-s --version");
            std::process::exit(2);
        }
    }
}
