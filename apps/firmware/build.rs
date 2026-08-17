use std::{env, path::PathBuf};

/// Points `LOCAL_CONFIG` at `config/local.env`, falling back to the committed
/// template when that file is absent.
///
/// `local.env` is gitignored, so it exists on a developer's machine and nowhere
/// else. Including it unconditionally meant CI -- and any fresh clone -- could
/// not so much as typecheck the firmware. The values only matter when the
/// firmware actually runs, so a build that cannot reach the network is still a
/// build worth having.
fn main() {
    let config = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("cargo sets this"))
        .join("../../config");
    let local = config.join("local.env");
    let template = config.join("local.env.example");

    // Watch both, so creating `local.env` later triggers a rebuild rather than
    // leaving a developer silently running against the template.
    println!("cargo:rerun-if-changed={}", local.display());
    println!("cargo:rerun-if-changed={}", template.display());

    let chosen = if local.exists() {
        local
    } else {
        println!(
            "cargo:warning=config/local.env not found, building against \
             config/local.env.example. This firmware will not reach your \
             network or your Linn."
        );
        template
    };

    println!("cargo:rustc-env=LOCAL_CONFIG_PATH={}", chosen.display());
}
