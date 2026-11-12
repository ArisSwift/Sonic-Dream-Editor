//! Build-time checks for the Raven link, plus the Linux runpath.
//!
//! Raven is a C library this crate links against, and when it cannot be found
//! the linker's own complaint is a wall of object files ending in
//! `unable to find library -lraven`. Every one of those reads the same to a
//! person. This checks first and says what is actually wrong.

use std::env;
use std::path::Path;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    // The default comes from .cargo/config.toml; an export overrides it.
    println!("cargo:rerun-if-env-changed=RAVEN_LIB_DIR");

    let target = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let names: &[&str] = match target.as_str() {
        "windows" => &["raven.lib", "raven.dll", "libraven.a"],
        _ => &["libraven.so", "libraven.a", "libraven.dylib"],
    };

    match env::var("RAVEN_LIB_DIR") {
        Err(_) => fail(&format!(
            "RAVEN_LIB_DIR is not set, so there is no folder to look for Raven in.\n\
             \n\
             Clone Raven beside this repository and build it, or point the variable\n\
             at a build you already have:\n\
             \n\
             \x20   RAVEN_LIB_DIR=/path/to/Raven/build cargo build\n\
             \n\
             Read me: Building, in README.md."
        )),
        Ok(dir) => {
            // A CMake build keeps the library in build/ on one generator and in
            // build/Release or build/bin/Release on another, so check as a
            // search path rather than one exact file.
            let found = search(Path::new(&dir), names);
            if !found {
                fail(&format!(
                    "RAVEN_LIB_DIR points at {dir}, but there is no {} in it.\n\
                     \n\
                     That folder is where a CMake build puts the Raven library. If the\n\
                     folder is empty, Raven has not been built yet:\n\
                     \n\
                     \x20   cmake -S Raven -B Raven/build -DCMAKE_BUILD_TYPE=Release \\\n\
                     \x20     -DRAVEN_BUILD_SHARED=ON\n\
                     \x20   cmake --build Raven/build --parallel\n\
                     \n\
                     Read me: Building, in README.md.",
                    names.join(", ")
                ));
            }
        }
    }

    // Linux: let the binary find Raven in its own folder.
    //
    // The release archive ships `libraven.so.0` beside `sdt-editor`, but a
    // shared library sitting next to an executable is not searched unless the
    // executable asks for it. `$ORIGIN` means "the folder I am in", so the
    // archive runs wherever it is unpacked. When the library is not there, the
    // loader falls back to `LD_LIBRARY_PATH`, which is how a development build
    // runs from `target/debug`.
    if target == "linux" {
        println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN");
    }
}

/// Whether any of `names` sits in `dir`, or in one of the folders a CMake
/// generator uses for a multi-config build.
fn search(dir: &Path, names: &[&str]) -> bool {
    let candidates = [
        dir.to_path_buf(),
        dir.join("Release"),
        dir.join("bin"),
        dir.join("bin/Release"),
    ];
    for folder in &candidates {
        if names.iter().any(|name| folder.join(name).is_file()) {
            return true;
        }
    }
    false
}

fn fail(message: &str) -> ! {
    // Printed rather than panicked: cargo already wraps this in its own error
    // block, and a panic adds a thread name and a source line around it.
    eprintln!("\n{message}\n");
    std::process::exit(1);
}
