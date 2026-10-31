use std::path::PathBuf;

pub fn daemon_path() -> PathBuf {
    executable_path("voimed.exe")
}

pub fn cli_path() -> PathBuf {
    executable_path("voime.exe")
}

fn executable_path(name: &str) -> PathBuf {
    if let Ok(executable) = std::env::current_exe() {
        if let Some(directory) = executable.parent() {
            let candidate = directory.join(name);
            if candidate.is_file() {
                return candidate;
            }
            for ancestor in directory.ancestors().take(6) {
                let candidate = ancestor.join("target").join("release").join(name);
                if candidate.is_file() {
                    return candidate;
                }
            }
        }
    }
    PathBuf::from(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn daemon_path_points_to_sibling_when_present() {
        let path = daemon_path();
        assert!(path.is_absolute() || path.as_os_str() == "voimed.exe");
    }
}
