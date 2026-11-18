//! Embeds the target triple this binary is built for, so the updater picks the release archive
//! of its own platform.

fn main() {
    let target = std::env::var("TARGET").unwrap_or_default();
    println!("cargo:rustc-env=HOOKAH_TARGET={target}");
    println!("cargo:rerun-if-changed=build.rs");
}
