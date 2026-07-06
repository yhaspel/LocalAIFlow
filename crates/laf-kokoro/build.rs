//! Link espeak-ng for grapheme→phoneme conversion (see g2p.rs).
//! Adapted from kokoroxide's build.rs; extended with pkg-config discovery and
//! an ESPEAK_NG_LIB_DIR override so packagers can point at a custom prefix.

fn main() {
    println!("cargo:rerun-if-env-changed=ESPEAK_NG_LIB_DIR");
    if let Ok(dir) = std::env::var("ESPEAK_NG_LIB_DIR") {
        println!("cargo:rustc-link-search=native={dir}");
        println!("cargo:rustc-link-lib=espeak-ng");
        return;
    }
    // Try pkg-config first (correct on most Linux distros).
    if pkg_config::Config::new().probe("espeak-ng").is_ok() {
        // probe() already emitted the link directives.
        return;
    }
    if cfg!(target_os = "macos") {
        // Homebrew default locations (Apple Silicon, then Intel).
        println!("cargo:rustc-link-search=native=/opt/homebrew/lib");
        println!("cargo:rustc-link-search=native=/usr/local/lib");
    }
    println!("cargo:rustc-link-lib=espeak-ng");
}
