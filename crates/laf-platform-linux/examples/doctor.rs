//! Headless doctor runner (dev/example only — the shipped app exposes the
//! same report via `local-ai-flow --doctor` and the Settings UI).
//! Useful for CI and containers: `cargo run -p laf-platform-linux --example doctor`

fn main() {
    let report = laf_platform_linux::doctor();
    println!("{}", report.to_terminal());
    std::process::exit(match report.worst() {
        laf_core::doctor::CheckStatus::Fail => 2,
        laf_core::doctor::CheckStatus::Warn => 1,
        laf_core::doctor::CheckStatus::Ok => 0,
    });
}
