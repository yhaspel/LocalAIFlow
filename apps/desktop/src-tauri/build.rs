fn main() {
    // Linux: Ubuntu's webkit2gtk references zlib symbols directly while only
    // declaring it transitively; modern GNU ld then demands -lz on the
    // command line. Scoped here (a global RUSTFLAGS would invalidate every
    // crate fingerprint).
    // Must be a trailing link-arg: an early `-lz` gets dropped by
    // `--as-needed` before webkit's undefined refs are seen.
    #[cfg(target_os = "linux")]
    println!("cargo:rustc-link-arg=-Wl,--no-as-needed,-lz");
    // Escape hatch for unusual link environments (containers/CI with
    // extracted-deb sysroots): space-separated extra linker args.
    println!("cargo:rerun-if-env-changed=LAF_EXTRA_LINK_ARGS");
    if let Ok(extra) = std::env::var("LAF_EXTRA_LINK_ARGS") {
        for arg in extra.split_whitespace() {
            println!("cargo:rustc-link-arg={arg}");
        }
    }

    tauri_build::build();
}
