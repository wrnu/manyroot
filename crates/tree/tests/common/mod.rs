//! Throwaway git repositories for import and materialization tests.

#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use tempfile::TempDir;

pub struct Repo {
    dir: TempDir,
}

impl Repo {
    /// An empty repository, isolated from the user's and system's git config,
    /// so signing, hooks and templates there can't interfere.
    pub fn new() -> Repo {
        let repo = Repo { dir: tempfile::tempdir().expect("a temporary directory") };
        repo.git(&["init", "-q", "-b", "main"]);
        repo
    }

    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    pub fn git(&self, args: &[&str]) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(self.path())
            .args(["-c", "user.name=test", "-c", "user.email=test@example.invalid", "-c", "commit.gpgsign=false"])
            .args(["-c", "core.hooksPath=/dev/null"])
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .output()
            .expect("git runs");
        assert!(output.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&output.stderr));
        String::from_utf8(output.stdout).expect("git output is UTF-8")
    }

    pub fn file(&self, path: &str) -> PathBuf {
        self.path().join(path)
    }

    pub fn write(&self, path: &str, bytes: &[u8]) {
        let file = self.file(path);
        fs::create_dir_all(file.parent().expect("a file has a parent")).expect("parent directories");
        fs::write(file, bytes).expect("writing a file");
    }

    pub fn executable(&self, path: &str, bytes: &[u8]) {
        use std::os::unix::fs::PermissionsExt;
        self.write(path, bytes);
        fs::set_permissions(self.file(path), fs::Permissions::from_mode(0o755)).expect("chmod");
    }

    pub fn symlink(&self, path: &str, target: &str) {
        let file = self.file(path);
        fs::create_dir_all(file.parent().expect("a file has a parent")).expect("parent directories");
        std::os::unix::fs::symlink(target, file).expect("creating a symlink");
    }

    /// Adds a path to the index directly, as a blob with these bytes, whatever
    /// the file system could hold.
    pub fn stage_blob(&self, path: &str, bytes: &[u8]) {
        let blob = self.file(".blob");
        fs::write(&blob, bytes).expect("writing a blob");
        let oid = self.git(&["hash-object", "-w", ".blob"]);
        fs::remove_file(&blob).expect("removing the blob file");
        self.git(&["update-index", "--add", "--cacheinfo", &format!("100644,{},{path}", oid.trim())]);
    }

    pub fn commit_all(&self) {
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "--allow-empty", "-m", "commit"]);
    }

    pub fn commit_index(&self) {
        self.git(&["commit", "-q", "--allow-empty", "-m", "commit"]);
    }
}
