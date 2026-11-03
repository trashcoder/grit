use super::*;
use std::{
    collections::BTreeSet,
    hash::{Hash, Hasher},
    time::{Duration, Instant},
};

#[derive(Default)]
pub(super) struct RepositoryWatcher {
    pending: Option<(PathBuf, Receiver<Result<u64, String>>)>,
    previous: Option<(PathBuf, u64)>,
    next_scan: Option<Instant>,
    pub paused: bool,
    pub error: Option<String>,
}

// Polling also works on network mounts and does not require a platform-specific watcher.
fn fingerprint(path: &Path) -> Result<u64, String> {
    let repo = Repository::open(path).map_err(|e| e.to_string())?;
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    format!("{:?}", repo.state()).hash(&mut hash);
    if let Ok(head) = repo.head() {
        head.name().hash(&mut hash);
        head.target().map(|id| id.to_string()).hash(&mut hash);
    }
    let mut refs: Vec<_> = repo
        .references()
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .map(|r| {
            (
                r.name().unwrap_or_default().to_owned(),
                r.target().map(|id| id.to_string()),
                r.symbolic_target().map(str::to_owned),
            )
        })
        .collect();
    refs.sort();
    refs.hash(&mut hash);
    let mut paths = BTreeSet::new();
    let index = repo.index().map_err(|e| e.to_string())?;
    for entry in index.iter() {
        entry.path.hash(&mut hash);
        entry.id.to_string().hash(&mut hash);
        entry.mode.hash(&mut hash);
        entry.flags.hash(&mut hash);
        if let (Some(root), Ok(relative)) = (repo.workdir(), std::str::from_utf8(&entry.path)) {
            paths.insert(root.join(relative));
        }
    }
    let mut options = StatusOptions::new();
    options.include_untracked(true).recurse_untracked_dirs(true);
    for entry in repo
        .statuses(Some(&mut options))
        .map_err(|e| e.to_string())?
        .iter()
    {
        entry.path_bytes().hash(&mut hash);
        entry.status().bits().hash(&mut hash);
        if let (Some(root), Some(relative)) = (repo.workdir(), entry.path()) {
            paths.insert(root.join(relative));
        }
    }
    for name in [
        "HEAD",
        "index",
        "config",
        "MERGE_HEAD",
        "CHERRY_PICK_HEAD",
        "REVERT_HEAD",
        "rebase-merge",
        "rebase-apply",
    ] {
        paths.insert(repo.path().join(name));
    }
    for path in paths {
        path.hash(&mut hash);
        match std::fs::symlink_metadata(&path) {
            Ok(meta) => {
                meta.len().hash(&mut hash);
                meta.modified().ok().hash(&mut hash);
                meta.file_type().is_symlink().hash(&mut hash);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::MetadataExt;
                    meta.mode().hash(&mut hash);
                    meta.ctime().hash(&mut hash);
                    meta.ctime_nsec().hash(&mut hash);
                }
            }
            Err(e) => e.kind().hash(&mut hash),
        }
    }
    Ok(hash.finish())
}

impl GritApp {
    pub(super) fn poll_repository_watcher(&mut self, ctx: &egui::Context) {
        if self.watcher.paused || self.path.is_none() {
            return;
        }
        let finished = self
            .watcher
            .pending
            .as_ref()
            .and_then(|(path, rx)| match rx.try_recv() {
                Ok(result) => Some((path.clone(), result)),
                Err(mpsc::TryRecvError::Disconnected) => {
                    Some((path.clone(), Err("Repository watcher stopped".into())))
                }
                Err(mpsc::TryRecvError::Empty) => None,
            });
        if let Some((path, result)) = finished {
            self.watcher.pending = None;
            if self.path.as_ref() == Some(&path) {
                match result {
                    Ok(value) => {
                        let changed = self
                            .watcher
                            .previous
                            .as_ref()
                            .is_some_and(|(old_path, old)| old_path == &path && *old != value);
                        // Keep the old baseline while a Git worker is running so the next scan refreshes.
                        if self.operation.is_none() {
                            self.watcher.previous = Some((path, value));
                            self.watcher.error = None;
                            if changed {
                                self.refresh();
                            }
                        }
                    }
                    Err(error) => self.watcher.error = Some(error),
                }
            }
        }
        if self.operation.is_none()
            && self.watcher.pending.is_none()
            && self.watcher.next_scan.is_none_or(|at| Instant::now() >= at)
        {
            let path = self.path.clone().unwrap();
            let worker_path = path.clone();
            let (tx, rx) = mpsc::channel();
            std::thread::spawn(move || {
                let _ = tx.send(fingerprint(&worker_path));
            });
            self.watcher.pending = Some((path, rx));
            self.watcher.next_scan = Some(Instant::now() + Duration::from_secs(2));
        }
        ctx.request_repaint_after(Duration::from_millis(if self.watcher.pending.is_some() {
            100
        } else {
            500
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn detects_worktree_index_refs_and_untracked_changes() {
        let root = std::env::temp_dir().join(format!(
            "grit-watcher-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let repo = Repository::init(&root).unwrap();
        std::fs::write(root.join("file"), "one").unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("file")).unwrap();
        index.write().unwrap();
        let tree_id = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let sig = git2::Signature::now("Watcher", "watcher@example.test").unwrap();
        let oid = repo
            .commit(Some("HEAD"), &sig, &sig, "first", &tree, &[])
            .unwrap();
        let initial = fingerprint(&root).unwrap();
        assert_eq!(initial, fingerprint(&root).unwrap());
        std::fs::write(root.join("file"), "longer changed content").unwrap();
        let dirty = fingerprint(&root).unwrap();
        assert_ne!(initial, dirty);
        std::fs::write(
            root.join("file"),
            "another modification while already dirty",
        )
        .unwrap();
        let dirty_again = fingerprint(&root).unwrap();
        assert_ne!(dirty, dirty_again);
        index.add_path(Path::new("file")).unwrap();
        index.write().unwrap();
        let staged = fingerprint(&root).unwrap();
        assert_ne!(dirty_again, staged);
        repo.reference("refs/heads/new", oid, false, "test")
            .unwrap();
        let branch = fingerprint(&root).unwrap();
        assert_ne!(staged, branch);
        std::fs::create_dir(root.join("nested")).unwrap();
        std::fs::write(root.join("nested/new"), "new").unwrap();
        let untracked = fingerprint(&root).unwrap();
        assert_ne!(branch, untracked);
        std::fs::remove_file(root.join("file")).unwrap();
        assert_ne!(untracked, fingerprint(&root).unwrap());
        let _ = std::fs::remove_dir_all(root);
    }
}
