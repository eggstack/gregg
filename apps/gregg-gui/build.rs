//! Build script for the Gregg Slint desktop frontend.
//!
//! `slint_build::compile` invokes the Slint compiler over the `.slint`
//! sources and writes the generated Rust into `OUT_DIR`, which
//! `slint::include_modules!()` then pulls in at compile time.
//!
//! The Slint compiler is a native binary shipped inside the `slint-build`
//! crate, so no system toolchain (cmake, clang, a C++ compiler) is required
//! for the software renderer used here.

fn main() {
    println!("cargo:rerun-if-changed=src/ui/main.slint");
    slint_build::compile("src/ui/main.slint").expect("failed to compile the Slint UI definition");
}
