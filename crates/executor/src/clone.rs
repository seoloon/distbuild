use std::path::{Path, PathBuf};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum CloneError {
    #[error("failed to clone {repo_url} (branch {branch}) into {dest}: {source}")]
    Clone {
        repo_url: String,
        branch: String,
        dest: PathBuf,
        #[source]
        source: git2::Error,
    },
}

/// Clones `repo_url` at `branch` into `dest`. `git2` is blocking, so the
/// actual clone runs on the blocking thread pool per PROMPT.md's rule
/// against blocking IO in async handlers.
pub async fn clone_repo(repo_url: &str, branch: &str, dest: &Path) -> Result<(), CloneError> {
    let repo_url = repo_url.to_string();
    let branch = branch.to_string();
    let dest = dest.to_path_buf();

    tokio::task::spawn_blocking(move || {
        let mut builder = git2::build::RepoBuilder::new();
        builder.branch(&branch);
        builder
            .clone(&repo_url, &dest)
            .map(|_repo| ())
            .map_err(|source| CloneError::Clone {
                repo_url,
                branch,
                dest,
                source,
            })
    })
    .await
    .expect("spawn_blocking task panicked")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn init_fixture_repo(dir: &Path) {
        let repo = git2::Repository::init(dir).expect("init fixture repo");
        fs::write(
            dir.join("Cargo.toml"),
            "[package]\nname=\"fixture\"\nversion=\"0.1.0\"\nedition=\"2021\"\n",
        )
        .expect("write Cargo.toml");
        fs::create_dir_all(dir.join("src")).expect("mkdir src");
        fs::write(
            dir.join("src").join("main.rs"),
            "fn main() { println!(\"hi\"); }\n",
        )
        .expect("write main.rs");

        let mut index = repo.index().expect("index");
        index
            .add_all(["*"], git2::IndexAddOption::DEFAULT, None)
            .expect("add_all");
        index.write().expect("write index");
        let tree_id = index.write_tree().expect("write_tree");
        let tree = repo.find_tree(tree_id).expect("find_tree");
        let sig = git2::Signature::now("Fixture", "fixture@example.com").expect("sig");
        repo.commit(Some("HEAD"), &sig, &sig, "initial", &tree, &[])
            .expect("commit");

        // `git2::Repository::init` defaults to "master" or "main" depending
        // on the installed git's `init.defaultBranch` — pin it explicitly
        // so the test is deterministic regardless of the host's git config.
        let branch_name = repo
            .head()
            .expect("head")
            .shorthand()
            .expect("shorthand")
            .to_string();
        if branch_name != "main" {
            let commit = repo.head().unwrap().peel_to_commit().unwrap();
            repo.branch("main", &commit, true).expect("branch main");
            repo.set_head("refs/heads/main").expect("set_head");
        }
    }

    #[tokio::test]
    async fn clones_a_local_repo_at_the_requested_branch() {
        let src = tempfile::tempdir().expect("tempdir");
        init_fixture_repo(src.path());

        let dest = tempfile::tempdir().expect("tempdir");
        let dest_path = dest.path().join("checkout");

        clone_repo(&src.path().to_string_lossy(), "main", &dest_path)
            .await
            .expect("clone_repo");

        assert!(dest_path.join("Cargo.toml").is_file());
        assert!(dest_path.join("src").join("main.rs").is_file());
    }

    #[tokio::test]
    async fn reports_an_error_for_a_nonexistent_remote() {
        let dest = tempfile::tempdir().expect("tempdir");
        let result = clone_repo(
            "/definitely/not/a/repo/path",
            "main",
            &dest.path().join("out"),
        )
        .await;
        assert!(result.is_err());
    }
}
