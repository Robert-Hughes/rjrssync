use std::{env, process::Command, path::{Path}, collections::HashMap};

/// There is logic that will be built into rjrssync to create a "big" binary from its
/// embedded resources, but we need a way of creating this initial big binary.
/// We'd like this to be done through the standard "cargo build" command, rather than wrapping
/// cargo in our own build script, which would be non-standard. Therefore we use this build.rs, 
/// which runs before cargo builds the rjrssync binary itself. This script runs cargo (again)
/// to build all of the "lite" binaries for each target platform we want to embed, and then 
/// gets these lite binaries embedded into the final binary, so that we end up with a big binary.
fn main() {
    // If this isn't a big binary build, then we have nothing to do.
    // We need this check otherwise we will recurse forever as we call into cargo to build the lite 
    // binaries, which will run this script.
    if env::var("CARGO_FEATURE_big_binary") != Ok("1".to_string()) {
        return;
    }

    // Build lite binaries for all supported targets
    //TODO: at the moment we're just building native - expand!
    // Note that we do need to include the lite binary for the native build, as this will be needed if 
    // the big binary is used to produce a new big binary for a different platform - that new big binary will 
    // need to have the lite binary for the native platform.
    // Technically we could get this by downgrading the big binary to a lite binary before embedding it, but this would 
    // be more complicated.
    let cargo = env::var("CARGO").unwrap(); // Use this to make sure we use the same cargo binary as the one we were called from (in case it isn't the default one)
    let lite_target_dir = Path::new(&env::var("OUT_DIR").unwrap()).join("lite");
    //TODO: pass through other arguments, like profile, features, etc.

    let mut cargo_cmd = Command::new(cargo);
    cargo_cmd.arg("build").arg("--bin").arg("rjrssync")
        // Disable the big_binary feature, so that this is a lite binary
        .arg("--no-default-features")
        // Build the lite binaries into a nested build folder
        .arg("--target-dir").arg(&lite_target_dir);

    // Prevent passing through environment variables that cargo has set for this build script.
    // This leads to problems because the build script that cargo will call would then see these env vars
    // which were not meant for it. Particularly the CARGO_FEATURE_big_binary var should NOT be set 
    // for the child build script, but it IS set for us, and so it would be inherited and cause an infinitely
    // recursive build!
    // We do however want to pass through other environment variables, as the user may have other stuff set 
    // that needs to be preserved.
    cargo_cmd.env_clear().envs(
        env::vars().filter(|&(ref v, _)| !v.starts_with("CARGO_")).collect::<HashMap<String, String>>());

    println!("Running {:?}", cargo_cmd);
    let cargo_status = cargo_cmd.status().expect("Failed to run cargo");
    assert!(cargo_status.success());

    let lite_binary = lite_target_dir.join("debug").join("rjrssync.exe"); //TODO: get this from cargo properly, .e.g https://zameermanji.com/blog/2021/6/17/embedding-a-rust-binary-in-another-rust-binary/

    // On Windows, we could embed the lite binaries as proper resources (Windows binaries have this concept),
    // but this isn't a thing on Linux, so we choose to use the same approach for both and so don't use this Windows feature.
    // Instead we append the embedded binaries as sections in the final binary (.exe/.elf) (both platforms
    // have the concept of sections in their executable formats). Because we'll need to manipulate the binaries
    // anyway at runtime when building a new big binary, we're gonna need to mess around with the sections anyway.

    // #[cfg(windows)]
    // {
    //     // Generate a .rc file to define the resources that we want to include in the Windows binary
    //     // First field is the resource name, then resource type (not sure how this matters, just using a custom string), then the path to the resource data
    //     // Need to escape the backslashes for the .rc file to be interpreted correctly
    //     let rc_contents = format!("EMBEDDED_LITE_BINARY BINARY \"{}\"", lite_binary.display().to_string().replace(r"\", r"\\"));
    //     //let rc_contents = format!("EMBEDDED_LITE_BINARY BINARY \"{}\"", r"D:\\Programming\\Utilities\\rjrssync\\.gitignore");
    //     let rc_file = Path::new(&env::var("OUT_DIR").unwrap()).join("embedded_binaries.rc");
    //     std::fs::write(&rc_file, rc_contents).expect("Failed to write rc file");

    //     embed_resource::compile_for(rc_file, &["rjrssync"]); // Build resources only into rjrssync (not any other binaries)
    // }

    //println!("cargo:rustc-link-arg=/OPT:NOREF");

    let embedded_binary_size = std::fs::metadata(&lite_binary).unwrap().len();

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
        //static EMBEDDED_DATA_TEST: [u8;16] = [ 0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15];
        static _EMBEDDED_DATA_TEST: [u8;{}] = *include_bytes!(r"{}");
    "#, embedded_binary_size, lite_binary.display().to_string());
    //let rc_contents = format!("EMBEDDED_LITE_BINARY BINARY \"{}\"", r"D:\\Programming\\Utilities\\rjrssync\\.gitignore");
    let generated_rs_file = Path::new(&env::var("OUT_DIR").unwrap()).join("embedded_binaries.rs");
    std::fs::write(&generated_rs_file, generated_rs_contents).expect("Failed to write generated rs file");

}

