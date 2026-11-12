//! Linux: let the binary find Raven in its own folder.
//!
//! The release archive ships `libraven.so.0` beside `sdt-editor`, but a
//! shared library sitting next to an executable is not searched unless the
//! executable asks for it. `$ORIGIN` means "the folder I am in", so the
//! archive runs wherever it is unpacked. When the library is not there, the
//! loader falls back to `LD_LIBRARY_PATH`, which is how a development build
//! runs from `target/debug`.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN");
    }
}
