// SPDX-License-Identifier: EPL-2.0

fn main() -> std::io::Result<()> {
    let protoc = protoc_bin_vendored::protoc_bin_path().expect("vendored protoc not available");
    let mut config = prost_build::Config::new();
    config.protoc_executable(protoc);
    config.compile_protos(
        &["../../proto/robocode_s/v1/robocode_s.proto"],
        &["../../proto"],
    )?;
    println!("cargo:rerun-if-changed=../../proto");
    Ok(())
}
