use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let Some(local_app_data) = std::env::var_os("LOCALAPPDATA") else {
        return;
    };
    let root = PathBuf::from(local_app_data).join("llcb");
    let Ok(entries) = std::fs::read_dir(&root) else {
        return;
    };
    let mut newest: Option<(SystemTime, PathBuf)> = None;
    for entry in entries.flatten() {
        let base = entry.path();
        let http = base.join("build/vendor/cpp-httplib/cpp-httplib.lib");
        let hash = base.join("build/vendor/hash/vendor-hash.lib");
        if !http.is_file() || !hash.is_file() {
            continue;
        }
        let modified = std::fs::metadata(&http)
            .and_then(|metadata| metadata.modified())
            .unwrap_or(UNIX_EPOCH);
        if newest
            .as_ref()
            .map(|(current, _)| modified > *current)
            .unwrap_or(true)
        {
            newest = Some((modified, base));
        }
    }
    let Some((_, base)) = newest else {
        return;
    };
    for (dir, lib) in [
        ("build/vendor/cpp-httplib", "cpp-httplib"),
        ("build/vendor/hash", "vendor-hash"),
    ] {
        let path = base.join(dir);
        if path.is_dir() {
            println!("cargo:rustc-link-search=native={}", path.display());
            println!("cargo:rustc-link-lib=static={lib}");
        }
    }
}
