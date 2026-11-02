use std::path::{Path, PathBuf};
use std::process::Command;
use std::{env, fs};

fn main() {
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows")
        || env::var("CARGO_CFG_TARGET_ENV").as_deref() != Ok("msvc")
    {
        return;
    }

    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let icon = manifest.join("../../assets/icons/voime.ico");
    if !icon.is_file() {
        return;
    }
    println!("cargo:rerun-if-changed={}", icon.display());

    let Some(rc) = find_rc() else {
        println!("cargo:warning=rc.exe not found; executable icon skipped");
        return;
    };
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let rc_file = out.join("voime.rc");
    let res_file = out.join("voime.res");
    let icon = icon.to_string_lossy().replace('\\', "\\\\");
    fs::write(&rc_file, format!("1 ICON \"{icon}\"\n")).expect("write icon resource script");

    match Command::new(rc)
        .arg("/nologo")
        .arg("/fo")
        .arg(&res_file)
        .arg(&rc_file)
        .status()
    {
        Ok(status) if status.success() => {
            println!("cargo:rustc-link-arg-bins={}", res_file.display());
        }
        Ok(status) => {
            println!("cargo:warning=rc.exe failed with {status}; executable icon skipped")
        }
        Err(error) => {
            println!("cargo:warning=failed to run rc.exe: {error}; executable icon skipped");
        }
    }
}

fn find_rc() -> Option<PathBuf> {
    if let Some(path) = env::var_os("RC") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }
    if let Ok(output) = Command::new("where").arg("rc.exe").output() {
        if output.status.success() {
            if let Some(line) = String::from_utf8_lossy(&output.stdout).lines().next() {
                let path = PathBuf::from(line.trim());
                if path.is_file() {
                    return Some(path);
                }
            }
        }
    }
    if let Some(path) = in_windows_kits() {
        return Some(path);
    }
    in_visual_studio()
}

fn in_windows_kits() -> Option<PathBuf> {
    let mut roots = Vec::new();
    if let Some(sdk) = env::var_os("WindowsSdkDir") {
        roots.push(PathBuf::from(sdk).join("bin"));
    }
    for variable in ["ProgramFiles(x86)", "ProgramFiles"] {
        if let Some(program_files) = env::var_os(variable) {
            roots.push(Path::new(&program_files).join("Windows Kits/10/bin"));
        }
    }
    for root in roots {
        for version in sorted_dirs(&root) {
            for arch in ["x64", "x86", "arm64"] {
                let candidate = version.join(arch).join("rc.exe");
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
    }
    None
}

fn in_visual_studio() -> Option<PathBuf> {
    let program_files = env::var_os("ProgramFiles")?;
    for year in ["2022", "2019"] {
        for edition in ["Community", "BuildTools", "Professional", "Enterprise"] {
            let tools = Path::new(&program_files)
                .join("Microsoft Visual Studio")
                .join(year)
                .join(edition)
                .join("VC/Tools/MSVC");
            for version in sorted_dirs(&tools) {
                for (host, arch) in [("Hostx64", "x64"), ("Hostx86", "x86")] {
                    let candidate = version.join("bin").join(host).join(arch).join("rc.exe");
                    if candidate.is_file() {
                        return Some(candidate);
                    }
                }
            }
        }
    }
    None
}

fn sorted_dirs(root: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut dirs: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    dirs.sort_by(|a, b| b.cmp(a));
    dirs
}
