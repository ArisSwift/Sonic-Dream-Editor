//! Build-time checks for the Raven link, plus how the binary finds the library.
//!
//! Raven is a C library this crate links against, and when it cannot be found
//! the linker's own complaint is a wall of object files ending in
//! `unable to find library -lraven`. Every one of those reads the same to a
//! person. This checks first and says what is actually wrong.

use std::env;
use std::path::{Path, PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    // The default comes from .cargo/config.toml; an export overrides it.
    println!("cargo:rerun-if-env-changed=RAVEN_LIB_DIR");

    let target = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let names: &[&str] = match target.as_str() {
        "windows" => &["raven.lib", "raven.dll", "libraven.a"],
        _ => &["libraven.so", "libraven.a", "libraven.dylib"],
    };

    let dir = match env::var("RAVEN_LIB_DIR") {
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
            let dir = PathBuf::from(dir);
            // A CMake build keeps the library in build/ under one generator and
            // in build/Release or build/bin/Release under another, so this is a
            // small search rather than one exact file.
            match find(&dir, names) {
                Some(_) => dir,
                None => fail(&format!(
                    "RAVEN_LIB_DIR points at {}, but there is no {} in it.\n\
                     \n\
                     That folder is where a CMake build puts the Raven library. If the\n\
                     folder is empty, Raven has not been built yet:\n\
                     \n\
                     \x20   cmake -S Raven -B Raven/build -DCMAKE_BUILD_TYPE=Release \\\n\
                     \x20     -DRAVEN_BUILD_SHARED=ON\n\
                     \x20   cmake --build Raven/build --parallel\n\
                     \n\
                     Read me: Building, in README.md.",
                    dir.display(),
                    names.join(", ")
                )),
            }
        }
    };

    // Linux: the binary asks the loader for libraven.so.0 and looks beside
    // itself first, so give it an $ORIGIN runpath. When the library is not
    // there, the loader falls back to LD_LIBRARY_PATH.
    if target == "linux" {
        println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN");
    }

    // Running the binary straight out of target/ should work too. A development
    // build has no library next to it, so put one there: a symlink on Linux, a
    // copy on Windows, where a symlink would need privileges it does not have.
    // This writes into the build output only, and `cargo clean` removes it.
    match target.as_str() {
        "linux" => link_beside(&dir),
        "windows" => {
            if let Some(dll) = find(&dir, &["raven.dll"]) {
                copy_beside(&dll, "raven.dll");
            }
        }
        _ => {}
    }
}

/// The first of `names` found in `dir` or in a folder a multi-config CMake
/// generator uses for it.
fn find(dir: &Path, names: &[&str]) -> Option<PathBuf> {
    let candidates = [
        dir.to_path_buf(),
        dir.join("Release"),
        dir.join("bin"),
        dir.join("bin/Release"),
    ];
    for folder in &candidates {
        for name in names {
            let path = folder.join(name);
            if path.is_file() {
                return Some(path);
            }
        }
    }
    None
}

/// `target/<profile>`, the folder the executable is written to.
fn binary_folder() -> PathBuf {
    let out = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    out.ancestors()
        .nth(3)
        .expect("target/<profile>")
        .to_path_buf()
}

/// `libraven.so.0.2.2` in `dir`, the file a symlink should point at.
#[cfg(target_os = "linux")]
fn versioned(dir: &Path) -> Option<PathBuf> {
    let mut found = None;
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with("libraven.so.") && name.len() > "libraven.so.".len() {
            found = Some(entry.path());
        }
    }
    found
}

/// The soname the linker recorded inside the library: `libraven.so.0`. Read out
/// of the file's own text so the build needs no `readelf`.
#[cfg(target_os = "linux")]
fn soname_name(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    let start = text.find("libraven.so.")?;
    let rest = &text[start..];
    let end = rest
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_'))
        .unwrap_or(rest.len());
    // Both "libraven.so.0" and "libraven.so.0.2.2" can appear in the file; the
    // soname is the first three parts either way.
    let mut parts = rest[..end].split('.');
    Some(format!("{}.{}.{}", parts.next()?, parts.next()?, parts.next()?))
}

#[cfg(target_os = "linux")]
fn link_beside(dir: &Path) {
    use std::os::unix::fs::symlink;

    let Some(real) = versioned(dir) else { return };
    let Some(soname) = soname_name(&real) else { return };

    let link = binary_folder().join(soname);
    if std::fs::read_link(&link).ok().as_deref() == Some(real.as_path()) {
        return;
    }
    let _ = std::fs::remove_file(&link);
    if symlink(&real, &link).is_err() {
        // A filesystem without symlinks is no reason to fail the build; the
        // binary still runs with LD_LIBRARY_PATH.
        println!(
            "cargo:warning=could not link {} beside the binary",
            link.display()
        );
    } else {
        println!("cargo:rerun-if-changed={}", real.display());
    }
}

#[cfg(not(target_os = "linux"))]
fn link_beside(_dir: &Path) {}

#[cfg(windows)]
fn copy_beside(source: &Path, name: &str) {
    if std::fs::copy(source, binary_folder().join(name)).is_ok() {
        println!("cargo:rerun-if-changed={}", source.display());
    }
}

#[cfg(not(windows))]
fn copy_beside(_source: &Path, _name: &str) {}

fn fail(message: &str) -> ! {
    // Printed rather than panicked: cargo already wraps this in its own error
    // block, and a panic adds a thread name and a source line around it.
    eprintln!("\n{message}\n");
    std::process::exit(1);
}
