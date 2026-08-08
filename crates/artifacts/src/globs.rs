use std::path::{Path, PathBuf};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum GlobError {
    #[error("invalid glob pattern `{pattern}`: {source}")]
    Pattern {
        pattern: String,
        #[source]
        source: glob::PatternError,
    },
}

/// Which family of default artifact globs to use, per PROMPT.md's
/// artifacts section (Tauri / Cargo / Node).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactKind {
    Tauri,
    Cargo,
    Node,
}

/// Returns the default glob patterns (relative to the repo root) for
/// `kind`, given the build `profile` and — for [`ArtifactKind::Cargo`] —
/// the compiled binary's name.
pub fn default_globs(kind: ArtifactKind, profile: &str, binary_name: &str) -> Vec<String> {
    match kind {
        ArtifactKind::Tauri => {
            const BUNDLE_EXTENSIONS: [&str; 7] =
                ["app", "dmg", "exe", "msi", "deb", "AppImage", "rpm"];
            BUNDLE_EXTENSIONS
                .iter()
                .map(|ext| format!("src-tauri/target/{profile}/bundle/**/*.{ext}"))
                .collect()
        }
        ArtifactKind::Cargo => {
            let binary = if cfg!(windows) {
                format!("{binary_name}.exe")
            } else {
                binary_name.to_string()
            };
            vec![format!("target/{profile}/{binary}")]
        }
        ArtifactKind::Node => vec![
            "dist/**/*".to_string(),
            "build/**/*".to_string(),
            "out/**/*".to_string(),
        ],
    }
}

/// Resolves `patterns` (relative to `base_dir`) against the filesystem and
/// returns every matching file (directories are skipped).
pub fn collect_artifacts(base_dir: &Path, patterns: &[String]) -> Result<Vec<PathBuf>, GlobError> {
    let mut results = Vec::new();
    for pattern in patterns {
        let full_pattern = base_dir.join(pattern);
        let full_pattern = full_pattern.to_string_lossy().into_owned();

        let paths = glob::glob(&full_pattern).map_err(|source| GlobError::Pattern {
            pattern: full_pattern.clone(),
            source,
        })?;

        for entry in paths.flatten() {
            if entry.is_file() {
                results.push(entry);
            }
        }
    }
    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn cargo_globs_append_exe_suffix_on_windows_only() {
        let globs = default_globs(ArtifactKind::Cargo, "release", "distbuild");
        let expected = if cfg!(windows) {
            "target/release/distbuild.exe"
        } else {
            "target/release/distbuild"
        };
        assert_eq!(globs, vec![expected.to_string()]);
    }

    #[test]
    fn tauri_globs_cover_all_bundle_extensions() {
        let globs = default_globs(ArtifactKind::Tauri, "release", "unused");
        assert_eq!(globs.len(), 7);
        assert!(globs.contains(&"src-tauri/target/release/bundle/**/*.dmg".to_string()));
        assert!(globs.contains(&"src-tauri/target/release/bundle/**/*.AppImage".to_string()));
    }

    #[test]
    fn node_globs_cover_dist_build_and_out() {
        let globs = default_globs(ArtifactKind::Node, "release", "unused");
        assert_eq!(
            globs,
            vec![
                "dist/**/*".to_string(),
                "build/**/*".to_string(),
                "out/**/*".to_string()
            ]
        );
    }

    #[test]
    fn collect_artifacts_finds_nested_files_and_skips_directories() {
        let dir = tempfile::tempdir().expect("tempdir");
        let nested = dir.path().join("dist").join("assets");
        fs::create_dir_all(&nested).expect("mkdir");
        fs::write(nested.join("app.js"), b"console.log('hi')").expect("write");
        fs::write(dir.path().join("dist").join("index.html"), b"<html></html>").expect("write");

        let patterns = default_globs(ArtifactKind::Node, "release", "unused");
        let mut found = collect_artifacts(dir.path(), &patterns).expect("collect");
        found.sort();

        assert_eq!(found.len(), 2);
        assert!(found.iter().all(|p| p.is_file()));
    }

    #[test]
    fn collect_artifacts_returns_empty_when_nothing_matches() {
        let dir = tempfile::tempdir().expect("tempdir");
        let patterns = default_globs(ArtifactKind::Cargo, "release", "nonexistent-binary");
        let found = collect_artifacts(dir.path(), &patterns).expect("collect");
        assert!(found.is_empty());
    }
}
