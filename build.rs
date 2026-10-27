use std::env;
use std::path::Path;
use std::process::Command;

fn main() {
    for input in [
        "web/src",
        "web/serve.html",
        "web/package.json",
        "web/pnpm-lock.yaml",
        "web/vite.config.ts",
    ] {
        println!("cargo::rerun-if-changed={input}");
    }

    if !Path::new("web/node_modules").is_dir() {
        panic!("the page is built from web/, which needs `pnpm install` there first");
    }
    let out = env::var("OUT_DIR").expect("cargo sets OUT_DIR");
    let built = Command::new("pnpm")
        .args(["exec", "vite", "build", "--emptyOutDir", "--outDir", &out])
        .current_dir("web")
        .output()
        .unwrap_or_else(|e| panic!("could not run pnpm to build the page: {e}"));
    if !built.status.success() {
        panic!(
            "building the page failed:\n{}{}",
            String::from_utf8_lossy(&built.stdout),
            String::from_utf8_lossy(&built.stderr)
        );
    }
}
