use std::{env, process::{Command}, path::{Path, PathBuf}, collections::HashMap};

/// Shared definition of the embedded binaries header.
#[path = "src/embedded_binaries.rs"]
mod embedded_binaries;

use embedded_binaries::*;

/// The set of target triples that we build and embed binaries for.
/// This means that we can deploy onto these platforms without needing to build from source.
/// This set depends on what targets are available, which depends on the build platform,
/// so is a function not a constant.
fn get_embedded_binary_target_triples() -> Vec<&'static str> {
    let mut result = vec![];

    // x64 Windows
    // There are two main target triples for this - one using MSVC and one using MinGW.
    // The MSVC one isn't available when building on Linux, so we have to use MinGW there.
    // When building on Windows, we could use the MinGW one too for consistency, but setting this up
    // on Windows is annoying (need to download MinGW as well as rustup target add), so we stick with MSVC.
    // (See section in notes.md for some more discussion on consistency of embedded binaries.)
    // Specifically, we check if the _target_ was already set to MSVC, implying that MSVC
    // is available, and accounting for somebody building on Windows but without MSVC.
    if std::env::var("TARGET").unwrap().contains("msvc") {
        result.push("x86_64-pc-windows-msvc");
    } else {
        result.push("x86_64-pc-windows-gnu");
    }

    // x64 Linux
    // Use musl rather than gnu as it's statically linked, so makes the resulting binary more portable
    result.push("x86_64-unknown-linux-musl");

    // aarch64 Linux
    // Use musl rather than gnu as it's statically linked, so makes the resulting binary more portable
    result.push("aarch64-unknown-linux-musl");

    result
}

/// There is logic in rjrssync to create a new "big" binary for a given target from its embedded binaries,
/// but we need a way of creating this initial big binary.
/// We'd like this to be done through the standard "cargo build" command, rather than wrapping
/// cargo in our own build script, which would be non-standard (hard to discover etc.).
/// Therefore we use this build.rs, which runs before cargo builds the rjrssync binary itself.
/// This script runs cargo (nested) to build all of the "lite" binaries for each target platform
/// we want to embed, and then gets these lite binaries embedded into the final binary,
/// so that we end up with a big binary.
/// This is called a "progenitor" binary and is controlled by a cargo feature flag.
fn main() {
    // Pass on the target triple env var to the proper build, so that we can access this when building
    // boss_deploy.rs (this isn't available there otherwise)
    println!("cargo:rustc-env=TARGET={}", std::env::var("TARGET").unwrap());

    // If this isn't a progenitor build, then we have nothing to do.
    // We need this check otherwise we will recurse forever as we call into cargo to build the lite
    // binaries, which will run this script.
    if env::var("CARGO_FEATURE_PROGENITOR") != Ok("1".to_string()) {
        return;
    }

    // Cargo's default behaviour is to re-run this script whenever any file in the package changes.
    // This is OK, but it results in all the embedded binaries being rebuilt even if only a test
    // file (etc.) is changed, slowing down incremental builds (embedded binaries don't depend on test code).
    // Instead, we tell cargo to run this script only when the main source changes
    // Note that we output different dependencies depending on CARGO_FEATURE_PROGENITOR. This seems
    // to work correctly (i.e. toggling this feature on and off along with incremental builds),
    // because cargo caches stuff in different folders based on the features used
    // (different folders with names like rjrssync-e000b82bf13fda43)
    println!("cargo:rerun-if-changed=.cargo");
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=Cargo.lock");
    println!("cargo:rerun-if-changed=Cargo.toml");

    // Build lite binaries for all supported targets
    // Use the same cargo binary as the one we were called from (in case it isn't the default one)
    let cargo = env::var("CARGO").unwrap();
    // Build the lite binaries into a nested build folder. We can put all the different target
    // builds into this same target folder, because cargo automatically makes a subfolder for each target triple
    let lite_target_dir = Path::new(&env::var("OUT_DIR").unwrap()).join("lite");
    let mut embedded_binaries = EmbeddedBinaries::default();
    for target_triple in get_embedded_binary_target_triples() {
        let mut cargo_cmd = Command::new(&cargo);
        cargo_cmd.arg("build")
            // For investigating build perf issues. (This won't be shown in the outer build unless "-vv" is specified there.)
            .arg("-v")
            // Build just the rjrssync binary, not any of the tests, examples etc.
            .arg("--bin=rjrssync")
            // Disable the progenitor feature, so that this is a lite binary
            .arg("--no-default-features")
            .arg(format!("--target={target_triple}"))
            .arg("--target-dir").arg(&lite_target_dir);

        // Match the debug/release-ness of the outer build. This is mainly so that debug builds are
        // faster (40s release down to 15s debug even when basically nothing has changed).
        if env::var("PROFILE").unwrap() == "release" {
            cargo_cmd.arg("--release");
        }

        // Match the profiling-ness of the outer build. Profiling on the boss requires profiling
        // on the doer too, so we need to be able to deploy profiling-enabled doers.
        if env::var("CARGO_FEATURE_PROFILING") == Ok("1".to_string()) {
            cargo_cmd.arg("--features=profiling");
        }

        // Prevent passing through environment variables that cargo has set for this build script.
        // This leads to problems because the build script that the nested cargo will call would
        // then see these env vars which were not meant for it.
        // Particularly the CARGO_FEATURE_PROGENITOR var should NOT be set for the child build script,
        // but it IS set for us, and so it would be inherited and cause an infinitely recursive build!
        // We do however want to pass through other environment variables, as the user/system may have other
        // stuff set that needs to be preserved.
        cargo_cmd.env_clear().envs(
            env::vars().filter(|&(ref v, _)| !v.starts_with("CARGO_")).collect::<HashMap<String, String>>());

        // Turn on logging that shows why things are being rebuilt. This is helpful for investigating
        // build performance. (This won't be shown in the outer build unless "-vv" is specified there.)
        // It might also be helpful to turn on this option for the outer build when investigating
        cargo_cmd
            .env("CARGO_LOG", "cargo::core::compiler::fingerprint=info");

        println!("Running {:?}", cargo_cmd);
        let cargo_status = cargo_cmd.status().expect("Failed to run cargo");
        assert!(cargo_status.success());

        // We need the filename of the executable that cargo built (something like target/release/rjrssync.exe),
        // but this isn't easily discoverable as it depends on debug vs release and possibly other cargo
        // implementation details that we don't  want to rely on.
        // The "proper" way of getting this is to use the JSON output from cargo, but this
        // is mutually exclusive with the regular (human) output. We want to keep the human output because
        // it may contain useful error messages, so unfortunately we now have to run cargo _again_, to get
        // the JSON output. This should be fast though, because the build is already done.
        cargo_cmd.arg("--message-format=json");
        println!("Running {:?}", cargo_cmd);
        let cargo_output = cargo_cmd.output().expect("Failed to run cargo (for JSON)");
        assert!(cargo_output.status.success());
        // The output is not actually a single JSON entity, but each line is a separate JSON object.
        let json_lines = &String::from_utf8_lossy(&cargo_output.stdout);
        let lite_binary_filename = {
            // Search the JSON output for the line that reports the executable path
            let mut lite_binary_file = None;
            for line in json_lines.lines() {
                let json = json::parse(&line).expect("Failed to parse JSON");
                if json["reason"] == "compiler-artifact" && json["target"]["name"] == "rjrssync" {
                    lite_binary_file = Some(json["executable"].as_str().unwrap().to_string());
                    break;
                }
            }
            PathBuf::from(lite_binary_file.expect("Couldn't find executable path in cargo JSON output"))
        };

        println!("{}", lite_binary_filename.display());
        let data = std::fs::read(lite_binary_filename).expect("Failed to read lite binary data");

        embedded_binaries.binaries.push(EmbeddedBinary {
            target_triple: target_triple.to_string(),
            data,
        });
    }

    // Serialize the binaries so they can be embedded into the binary we are building.
    // Save it to a file so that it can be included into the final binary using include_bytes!
    let embedded_binaries_filename = Path::new(&env::var("OUT_DIR").unwrap()).join("embedded_binaries.bin");
    {
        let f = std::fs::File::create(&embedded_binaries_filename).expect("Failed to create file");
        bincode::serialize_into(f, &embedded_binaries).expect("Failed to serialize");
    }
    let embedded_binaries_size = std::fs::metadata(&embedded_binaries_filename).unwrap().len();

    // Generate an .rs file that includes the contents of this file into the final binary, in a
    // specially named section of the executable. This is include!'d from boss_deploy.rs.
    // We also need to have a proper reference to the data, otherwise the compiler/linker will optimise it out,
    // which is also done in boss_deploy.rs.
    let section_name = embedded_binaries::SECTION_NAME;
    let generated_rs_contents = format!(
        r#"
// This file is generated by build.rs
#[link_section = "{section_name}"]
static EMBEDDED_BINARIES_DATA: [u8;{embedded_binaries_size}] = *include_bytes!(r"{}");
        "#, embedded_binaries_filename.display());
    let generated_rs_filename = Path::new(&env::var("OUT_DIR").unwrap()).join("embedded_binaries.rs");
    std::fs::write(&generated_rs_filename, generated_rs_contents).expect("Failed to write generated rs file");
}

