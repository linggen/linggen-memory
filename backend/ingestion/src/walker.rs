use anyhow::Result;
use ignore::WalkBuilder;
use std::path::{Path, PathBuf};
use tracing::info;

pub struct FileWalker {
    root: PathBuf,
}

impl FileWalker {
    pub fn new(root: impl AsRef<Path>) -> Self {
        Self {
            root: root.as_ref().to_path_buf(),
        }
    }

    pub fn walk(&self) -> Result<Vec<PathBuf>> {
        info!("Scanning directory: {:?}", self.root);
        let mut files = Vec::new();

        let walker = WalkBuilder::new(&self.root)
            .hidden(true) // Ignore hidden files (like .git)
            .git_ignore(true) // Respect .gitignore
            .build();

        for result in walker {
            match result {
                Ok(entry) => {
                    if entry.file_type().map(|ft| ft.is_file()).unwrap_or(false) {
                        let path = entry.path();

                        // explicit check to skip .git if hidden(false) is ever re-enabled
                        if path.components().any(|c| c.as_os_str() == ".git") {
                            continue;
                        }

                        files.push(path.to_path_buf());
                    }
                }
                Err(err) => {
                    tracing::warn!("Error walking directory: {}", err);
                }
            }
        }

        info!("Found {} files", files.len());
        Ok(files)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_walker_basic() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("file1.rs"), "fn main() {}").unwrap();
        fs::write(dir.path().join("file2.md"), "# README").unwrap();

        let walker = FileWalker::new(dir.path());
        let files = walker.walk().unwrap();
        assert_eq!(files.len(), 2);
    }

    #[test]
    fn test_walker_skips_git_dir() {
        let dir = tempfile::tempdir().unwrap();
        let git_dir = dir.path().join(".git");
        fs::create_dir(&git_dir).unwrap();
        fs::write(git_dir.join("config"), "git config").unwrap();
        fs::write(dir.path().join("main.rs"), "code").unwrap();

        let walker = FileWalker::new(dir.path());
        let files = walker.walk().unwrap();
        // Should only find main.rs, not .git/config
        assert_eq!(files.len(), 1);
        assert!(files[0].ends_with("main.rs"));
    }

    #[test]
    fn test_walker_respects_gitignore() {
        let dir = tempfile::tempdir().unwrap();
        // Create .gitignore
        fs::write(dir.path().join(".gitignore"), "*.log\ntarget/\n").unwrap();
        fs::write(dir.path().join("app.rs"), "code").unwrap();
        fs::write(dir.path().join("debug.log"), "logs").unwrap();
        let target_dir = dir.path().join("target");
        fs::create_dir(&target_dir).unwrap();
        fs::write(target_dir.join("output.o"), "binary").unwrap();

        // Initialize a git repo so .gitignore is respected
        fs::create_dir(dir.path().join(".git")).unwrap();

        let walker = FileWalker::new(dir.path());
        let files = walker.walk().unwrap();

        let file_names: Vec<String> = files
            .iter()
            .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
            .collect();

        assert!(file_names.contains(&"app.rs".to_string()));
        // .gitignore is a hidden file and the walker skips hidden files
        assert!(!file_names.contains(&".gitignore".to_string()));
        assert!(!file_names.contains(&"debug.log".to_string()));
        assert!(!file_names.contains(&"output.o".to_string()));
    }

    #[test]
    fn test_walker_nested_directories() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("src").join("deep");
        fs::create_dir_all(&nested).unwrap();
        fs::write(nested.join("mod.rs"), "mod deep;").unwrap();
        fs::write(dir.path().join("lib.rs"), "pub mod src;").unwrap();

        let walker = FileWalker::new(dir.path());
        let files = walker.walk().unwrap();
        assert_eq!(files.len(), 2);
    }

    #[test]
    fn test_walker_empty_directory() {
        let dir = tempfile::tempdir().unwrap();
        let walker = FileWalker::new(dir.path());
        let files = walker.walk().unwrap();
        assert!(files.is_empty());
    }

    #[test]
    fn test_walker_only_directories() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("subdir1")).unwrap();
        fs::create_dir(dir.path().join("subdir2")).unwrap();

        let walker = FileWalker::new(dir.path());
        let files = walker.walk().unwrap();
        assert!(files.is_empty()); // walker only returns files, not dirs
    }
}
