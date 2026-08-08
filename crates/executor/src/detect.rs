use std::path::Path;

/// The build system detected for a project directory, in PROMPT.md's
/// priority order (checked top to bottom; first match wins).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildSystem {
    /// `distbuild.toml` present — full override. Command/env/artifact globs
    /// come from parsing the file itself; see Phase 4's `distbuild.toml`
    /// parser. Detection only confirms the file exists.
    DistbuildToml,
    Tauri,
    Cargo,
    Bun,
    Pnpm,
    Npm,
    Make,
}

impl BuildSystem {
    /// The ordered (program, args) steps to run for this build system in
    /// `profile`. Returns `None` for [`BuildSystem::DistbuildToml`], whose
    /// steps are defined by the parsed file's `command` field (Phase 4).
    pub fn steps(&self, profile: &str) -> Option<Vec<(String, Vec<String>)>> {
        match self {
            BuildSystem::DistbuildToml => None,
            BuildSystem::Tauri => Some(vec![(
                "cargo".to_string(),
                vec!["tauri".to_string(), "build".to_string()],
            )]),
            BuildSystem::Cargo => {
                let mut args = vec!["build".to_string()];
                if profile.eq_ignore_ascii_case("release") {
                    args.push("--release".to_string());
                }
                Some(vec![("cargo".to_string(), args)])
            }
            BuildSystem::Bun => Some(vec![
                ("bun".to_string(), vec!["install".to_string()]),
                (
                    "bun".to_string(),
                    vec!["run".to_string(), "build".to_string()],
                ),
            ]),
            BuildSystem::Pnpm => Some(vec![
                ("pnpm".to_string(), vec!["install".to_string()]),
                ("pnpm".to_string(), vec!["build".to_string()]),
            ]),
            BuildSystem::Npm => Some(vec![
                ("npm".to_string(), vec!["ci".to_string()]),
                (
                    "npm".to_string(),
                    vec!["run".to_string(), "build".to_string()],
                ),
            ]),
            BuildSystem::Make => Some(vec![("make".to_string(), vec!["build".to_string()])]),
        }
    }
}

/// Detects which build system a cloned repository uses, checking in
/// PROMPT.md's priority order: `distbuild.toml` > Tauri > Cargo > Bun >
/// pnpm > npm > Make. Returns `None` if no recognized marker file exists.
pub fn detect_build_system(project_dir: &Path) -> Option<BuildSystem> {
    if project_dir.join("distbuild.toml").is_file() {
        return Some(BuildSystem::DistbuildToml);
    }
    if project_dir
        .join("src-tauri")
        .join("tauri.conf.json")
        .is_file()
    {
        return Some(BuildSystem::Tauri);
    }
    if project_dir.join("Cargo.toml").is_file() {
        return Some(BuildSystem::Cargo);
    }
    if project_dir.join("bun.lockb").is_file() {
        return Some(BuildSystem::Bun);
    }
    if project_dir.join("pnpm-lock.yaml").is_file() {
        return Some(BuildSystem::Pnpm);
    }
    if project_dir.join("package.json").is_file() {
        return Some(BuildSystem::Npm);
    }
    if project_dir.join("Makefile").is_file() {
        return Some(BuildSystem::Make);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn touch(dir: &Path, name: &str) {
        fs::write(dir.join(name), b"").expect("failed to write marker file");
    }

    #[test]
    fn detects_distbuild_toml_first() {
        let dir = tempfile::tempdir().expect("tempdir");
        touch(dir.path(), "distbuild.toml");
        touch(dir.path(), "Cargo.toml");
        assert_eq!(
            detect_build_system(dir.path()),
            Some(BuildSystem::DistbuildToml)
        );
    }

    #[test]
    fn detects_tauri_before_plain_cargo() {
        let dir = tempfile::tempdir().expect("tempdir");
        touch(dir.path(), "Cargo.toml");
        fs::create_dir_all(dir.path().join("src-tauri")).expect("mkdir");
        touch(&dir.path().join("src-tauri"), "tauri.conf.json");
        assert_eq!(detect_build_system(dir.path()), Some(BuildSystem::Tauri));
    }

    #[test]
    fn detects_plain_cargo() {
        let dir = tempfile::tempdir().expect("tempdir");
        touch(dir.path(), "Cargo.toml");
        assert_eq!(detect_build_system(dir.path()), Some(BuildSystem::Cargo));
    }

    #[test]
    fn detects_bun_before_pnpm_and_npm() {
        let dir = tempfile::tempdir().expect("tempdir");
        touch(dir.path(), "bun.lockb");
        touch(dir.path(), "pnpm-lock.yaml");
        touch(dir.path(), "package.json");
        assert_eq!(detect_build_system(dir.path()), Some(BuildSystem::Bun));
    }

    #[test]
    fn detects_pnpm_before_npm() {
        let dir = tempfile::tempdir().expect("tempdir");
        touch(dir.path(), "pnpm-lock.yaml");
        touch(dir.path(), "package.json");
        assert_eq!(detect_build_system(dir.path()), Some(BuildSystem::Pnpm));
    }

    #[test]
    fn detects_npm_from_package_json_alone() {
        let dir = tempfile::tempdir().expect("tempdir");
        touch(dir.path(), "package.json");
        assert_eq!(detect_build_system(dir.path()), Some(BuildSystem::Npm));
    }

    #[test]
    fn detects_make_as_last_resort() {
        let dir = tempfile::tempdir().expect("tempdir");
        touch(dir.path(), "Makefile");
        assert_eq!(detect_build_system(dir.path()), Some(BuildSystem::Make));
    }

    #[test]
    fn returns_none_when_nothing_recognized() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert_eq!(detect_build_system(dir.path()), None);
    }

    #[test]
    fn cargo_steps_add_release_flag_only_for_release_profile() {
        let debug_steps = BuildSystem::Cargo.steps("debug").expect("steps");
        assert_eq!(
            debug_steps,
            vec![("cargo".to_string(), vec!["build".to_string()])]
        );

        let release_steps = BuildSystem::Cargo.steps("release").expect("steps");
        assert_eq!(
            release_steps,
            vec![(
                "cargo".to_string(),
                vec!["build".to_string(), "--release".to_string()]
            )]
        );
    }

    #[test]
    fn distbuild_toml_has_no_builtin_steps() {
        assert_eq!(BuildSystem::DistbuildToml.steps("release"), None);
    }
}
