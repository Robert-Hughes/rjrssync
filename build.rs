use std::{env, process::{Command}, path::{Path, PathBuf}, collections::HashMap};

/// Shared definition of the embedded binaries header.
#[path = "src/embedded_binaries.rs"]
mod embedded_binaries;

use embedded_binaries::EmbeddedBinaries;
use embedded_binaries::EmbeddedBinary;

/// The set of target triples that we build and embed binaries for.
/// This means that we can deploy onto these platforms without needing
/// to build from source.
//TODO: should we always include the target that is currently being built for?
// e.g. if the build is targeting x86_64-unknown-linux-gnu, should we include that as well or instead
// of the -musl variant?
// For windows, we can't target msvc when building on Linux, we have to use the -gnu one.
// For consistency, should we target this when building on Windows as well?
const EMBEDDED_BINARY_TARGET_TRIPLES: &[&str] = &[
    "x86_64-pc-windows-msvc",
    "x86_64-unknown-linux-musl", // Use musl rather than gnu as it's statically linked, so makes the resulting binary more portable
    "aarch64-unknown-linux-musl",
    //TODO: building progenitor on Linux didn't work because needs MSVC linker, use mingw? (-gnu suffix?)
    //TODO: building progenitor cross compiling on Windoiws for aarch64, worked but the big binary produced seemed to be missing the embedded binaries section. Maybe the linker is removing it?
];

//TODO: how does this work when building on Linux - can we cross compile from Linux to Windows? :O

/// There is logic that will be built into rjrssync to create a "big" binary from its
/// embedded resources, but we need a way of creating this initial big binary.
/// We'd like this to be done through the standard "cargo build" command, rather than wrapping
/// cargo in our own build script, which would be non-standard. Therefore we use this build.rs, 
/// which runs before cargo builds the rjrssync binary itself. This script runs cargo (again)
/// to build all of the "lite" binaries for each target platform we want to embed, and then 
/// gets these lite binaries embedded into the final binary, so that we end up with a big binary.
fn main() {
    //TODO: set appropriate rebuild options (by printing output for cargo),
    // so that this is rebuilt appropriately (but not too much). Maybe the default of whenever a file in the 
    // package changes is fine, because we're rebuilding the whole package here anyway (for different targets)...

    // Pass on the target triple env var so that we can access this when compiling the main program
    // (this isn't available there otherwise)
    println!("cargo:rustc-env=TARGET={}", std::env::var("TARGET").unwrap());

    // If this isn't a big binary build, then we have nothing to do.
    // We need this check otherwise we will recurse forever as we call into cargo to build the lite 
    // binaries, which will run this script.
    if env::var("CARGO_FEATURE_PROGENITOR") != Ok("1".to_string()) {
        return;
    }

    // Build lite binaries for all supported targets
    // Note that we do need to include the lite binary for the native build, as this will be needed if 
    // the big binary is used to produce a new big binary for a different platform - that new big binary will 
    // need to have the lite binary for the native platform.
    // Technically we could get this by downgrading the big binary to a lite binary before embedding it, but this would 
    // be more complicated.
    let cargo = env::var("CARGO").unwrap(); // Use this to make sure we use the same cargo binary as the one we were called from (in case it isn't the default one)
    // Build the lite binaries into a nested build folder. We can put all the different target
    // builds into this same target folder, because cargo automatically makes a subfolder for each target
    //TODO: even though this works, cargo seems to be doing rebuilds when nothing has changed, so maybe 
    // putting them in separate target folders would lead to better rebuild behaviour?
    let lite_target_dir = Path::new(&env::var("OUT_DIR").unwrap()).join("lite");
    let mut embedded_binaries = EmbeddedBinaries::default();
    for target_triple in EMBEDDED_BINARY_TARGET_TRIPLES {
        //TODO: this should be a release build? Or should it match the binary being built...?
        // for remote source builds, we still always build release for the remote so not sure.
        let mut cargo_cmd = Command::new(&cargo);
        cargo_cmd.arg("build").arg("-r").arg("--bin").arg("rjrssync")
            // Disable the progenitor feature, so that this is a lite binary
            .arg("--no-default-features")
            .arg(format!("--target={target_triple}"))
            .arg("--target-dir").arg(&lite_target_dir);
        //TODO: pass through other arguments, like profile, features, etc.

        // Prevent passing through environment variables that cargo has set for this build script.
        // This leads to problems because the build script that cargo will call would then see these env vars
        // which were not meant for it. Particularly the CARGO_FEATURE_progenitor var should NOT be set 
        // for the child build script, but it IS set for us, and so it would be inherited and cause an infinitely
        // recursive build!
        // We do however want to pass through other environment variables, as the user may have other stuff set 
        // that needs to be preserved.
        cargo_cmd.env_clear().envs(
            env::vars().filter(|&(ref v, _)| !v.starts_with("CARGO_")).collect::<HashMap<String, String>>());

        //TODO: if the target platform cross-compiler isn't installed, then the build will produce a LOT of
        // errors which is very noisy and slow. Maybe instead we should do our own quick check up front?
        println!("Running {:?}", cargo_cmd);
        let cargo_status = cargo_cmd.status().expect("Failed to run cargo");
        assert!(cargo_status.success());

        // We need the filename of the executable that cargo built, but this isn't easily findable as
        // it depends on debug vs release and possibly other cargo implementation details that we don't 
        // want to rely on. The "proper" way of getting this is to use the JSON output from cargo, but this
        // is mutually exclusive with the regular (human) output. We want to keep the human output because
        // it will contain useful error messages, so unfortunately we now have to run cargo _again_, to get 
        // the JSON output. This should be fast though, because the build is already done.
        cargo_cmd.arg("--message-format=json");
        println!("Running {:?}", cargo_cmd);
        let cargo_output = cargo_cmd.output().expect("Failed to run cargo");
        assert!(cargo_output.status.success());
        let json = &String::from_utf8_lossy(&cargo_output.stdout);
       // println!("{}", json);
        let lite_binary_file = {
            let mut lite_binary_file = None;
            for line in json.lines() {
                let json = json::parse(&line).expect("Failed to parse JSON");
                if json["reason"] == "compiler-artifact" && 
                    //json["package_id"].as_str().unwrap_or_default().starts_with("rjrssync ") &&
                    json["target"]["name"].as_str().unwrap_or_default() == "rjrssync"{
                        // println!("{}", json);
                        lite_binary_file = Some(json["executable"].as_str().unwrap().to_string());
                        break;
                }
            }
            PathBuf::from(lite_binary_file.unwrap())
        };

        println!("{}", lite_binary_file.display());
        let data = std::fs::read(lite_binary_file).expect("Failed to read binary");

        embedded_binaries.binaries.push(EmbeddedBinary { 
            target_triple: target_triple.to_string(),
            data,
        });
    }

    // On Windows, we could embed the lite binaries as proper resources (Windows binaries have this concept),
    // but this isn't a thing on Linux, so we choose to use the same approach for both and so don't use this Windows feature.
    // Instead we append the embedded binaries as sections in the final binary (.exe/.elf) (both platforms
    // have the concept of sections in their executable formats). Because we'll need to manipulate the binaries
    // anyway at runtime when building a new big binary, we're gonna need to mess around with the sections anyway.

    // Serialize the binaries so they can be embedded into the binary we are building.
    // Save it to a file so that it can be included into the final binary using include_bytes!
    let embedded_binaries_filename = Path::new(&env::var("OUT_DIR").unwrap()).join("embedded_binaries.bin");
    {
        let f = std::fs::File::create(&embedded_binaries_filename).expect("Failed to create file");
        bincode::serialize_into(f, &embedded_binaries).expect("Failed to serialize");
    }
    let embedded_binaries_size = std::fs::metadata(&embedded_binaries_filename).unwrap().len();

    // Generate an .rs file that includes the contents of this file into the final binary, in a 
    // specially named section of the executable. This is included from boss_deploy.rs.
    let generated_rs_contents = format!(
    r#"
        // Put it in a section that won't be optimised out (special name for MSVC, for resources,
        // even though we are not actually using resources!)
        #[link_section = ".rsrc1"] 
        // We don't actually use this symbol anywhere - it's only used to get the embedded data into
        // the big binary during the cargo build. When we need this data, we read it directly from the 
        // exe, because this symbol won't be available in the lite binary build (for deploying from an already-deployed
        // binary)
        #[used]
        static _EMBEDDED_DATA_TEST: [u8;{}] = *include_bytes!(r"{}");
    "#, embedded_binaries_size, embedded_binaries_filename.display());
    //let rc_contents = format!("EMBEDDED_LITE_BINARY BINARY \"{}\"", r"D:\\Programming\\Utilities\\rjrssync\\.gitignore");
    let generated_rs_file = Path::new(&env::var("OUT_DIR").unwrap()).join("embedded_binaries.rs");
    std::fs::write(&generated_rs_file, generated_rs_contents).expect("Failed to write generated rs file");
}

