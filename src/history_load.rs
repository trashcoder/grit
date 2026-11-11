use std::{
    path::PathBuf,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Receiver},
    },
    thread,
};

use crate::Commit;

struct Request {
    token: u64,
    path: PathBuf,
    signature: String,
}
#[derive(Default)]
struct QueueState {
    request: Option<Request>,
    shutdown: bool,
}
pub(super) struct ResultSet {
    pub token: u64,
    pub path: PathBuf,
    pub signature: String,
    pub result: Result<Vec<Commit>, String>,
}

pub(super) struct HistoryLoad {
    queue: Arc<(Mutex<QueueState>, Condvar)>,
    rx: Receiver<ResultSet>,
    next_token: u64,
    pending: Option<(u64, PathBuf, String)>,
    loaded: Option<(PathBuf, String)>,
    latest_token: Arc<AtomicU64>,
}

impl Default for HistoryLoad {
    fn default() -> Self {
        let queue = Arc::new((Mutex::new(QueueState::default()), Condvar::new()));
        let worker_queue = Arc::clone(&queue);
        let latest_token = Arc::new(AtomicU64::new(0));
        let worker_token = Arc::clone(&latest_token);
        let (results, rx) = mpsc::channel();
        thread::Builder::new()
            .name("grit-history-loader".into())
            .spawn(move || {
                loop {
                    let request = {
                        let (lock, wake) = &*worker_queue;
                        let mut item = lock.lock().expect("history queue lock");
                        while item.request.is_none() && !item.shutdown {
                            item = wake.wait(item).expect("history queue wait");
                        }
                        if item.shutdown {
                            return;
                        }
                        item.request.take().expect("queued history request")
                    };
                    let token = request.token;
                    let result = std::panic::catch_unwind(|| {
                        let repo = git2::Repository::open(&request.path)
                            .map_err(|error| error.message().to_owned())?;
                        crate::history_with_cancel(&repo, || {
                            worker_token.load(Ordering::Relaxed) != token
                        })
                    })
                    .unwrap_or_else(|_| Err("History loading stopped unexpectedly".into()));
                    if results
                        .send(ResultSet {
                            token: request.token,
                            path: request.path,
                            signature: request.signature,
                            result,
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            })
            .expect("history worker thread");
        Self {
            queue,
            rx,
            next_token: 0,
            pending: None,
            loaded: None,
            latest_token,
        }
    }
}

impl HistoryLoad {
    pub(super) fn request(&mut self, path: PathBuf, signature: String) {
        if self
            .pending
            .as_ref()
            .is_some_and(|(_, p, s)| p == &path && s == &signature)
        {
            return;
        }
        if self
            .loaded
            .as_ref()
            .is_some_and(|(p, s)| p == &path && s == &signature)
        {
            if self.pending.is_some() {
                self.next_token = self.next_token.wrapping_add(1);
                self.latest_token.store(self.next_token, Ordering::Relaxed);
                self.pending = None;
                let (lock, _) = &*self.queue;
                lock.lock().expect("history queue lock").request = None;
            }
            return;
        }
        self.next_token = self.next_token.wrapping_add(1);
        let token = self.next_token;
        self.latest_token.store(token, Ordering::Relaxed);
        let request = Request {
            token,
            path: path.clone(),
            signature: signature.clone(),
        };
        // Replacing the single queue slot bounds work and always preserves the latest state.
        let (lock, wake) = &*self.queue;
        lock.lock().expect("history queue lock").request = Some(request);
        wake.notify_one();
        self.pending = Some((token, path, signature));
    }

    pub(super) fn poll(&mut self) -> Option<ResultSet> {
        let mut newest = None;
        while let Ok(result) = self.rx.try_recv() {
            if self.pending.as_ref().is_some_and(|(token, path, sig)| {
                *token == result.token && path == &result.path && sig == &result.signature
            }) {
                self.pending = None;
                if result.result.is_ok() {
                    self.loaded = Some((result.path.clone(), result.signature.clone()));
                }
                newest = Some(result);
            }
        }
        newest
    }

    pub(super) fn invalidate(&mut self) {
        self.next_token = self.next_token.wrapping_add(1);
        self.latest_token.store(self.next_token, Ordering::Relaxed);
        self.pending = None;
        self.loaded = None;
        // Clear a queued, not-yet-running request so a repository switch cannot delay
        // the current repository behind stale work.
        let (lock, _) = &*self.queue;
        lock.lock().expect("history queue lock").request = None;
    }

    pub(super) fn is_pending(&self) -> bool {
        self.pending.is_some()
    }
}

impl Drop for HistoryLoad {
    fn drop(&mut self) {
        let (lock, wake) = &*self.queue;
        let mut state = lock.lock().expect("history queue lock");
        state.shutdown = true;
        state.request = None;
        wake.notify_one();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        path::Path,
        sync::atomic::{AtomicU64, Ordering},
        time::{Duration, Instant},
    };

    static NEXT_REPO: AtomicU64 = AtomicU64::new(0);

    struct TempRepo(PathBuf);

    impl TempRepo {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "grit-history-loader-{}-{}",
                std::process::id(),
                NEXT_REPO.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn with_commit() -> (Self, String) {
            let dir = Self::new();
            let repo = git2::Repository::init(&dir.0).unwrap();
            std::fs::write(dir.0.join("file.txt"), "history\n").unwrap();
            let mut index = repo.index().unwrap();
            index.add_path(Path::new("file.txt")).unwrap();
            index.write().unwrap();
            let tree_id = index.write_tree().unwrap();
            let tree = repo.find_tree(tree_id).unwrap();
            let sig = git2::Signature::now("test", "test@example.invalid").unwrap();
            let id = repo
                .commit(Some("HEAD"), &sig, &sig, "test commit", &tree, &[])
                .unwrap();
            (dir, id.to_string())
        }
    }

    impl Drop for TempRepo {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn wait_result(loader: &mut HistoryLoad) -> ResultSet {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(result) = loader.poll() {
                return result;
            }
            assert!(Instant::now() < deadline, "history worker timed out");
            thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn latest_request_wins_and_traversal_cancels_stale_job() {
        let (repo, commit) = TempRepo::with_commit();
        let mut loader = HistoryLoad::default();
        loader.request(PathBuf::from("/missing/grit-history-repo"), "old".into());
        loader.request(repo.0.clone(), "latest".into());

        let result = wait_result(&mut loader);
        assert_eq!(result.path, repo.0);
        let commits = result.result.unwrap();
        assert!(commits.iter().any(|entry| entry.id == commit));
        assert!(!loader.is_pending());
    }

    #[test]
    fn missing_repository_reports_error_and_clears_pending_state() {
        let mut loader = HistoryLoad::default();
        loader.request(
            PathBuf::from("/missing/grit-history-repo"),
            "missing".into(),
        );
        let result = wait_result(&mut loader);
        assert!(result.result.is_err());
        assert!(!loader.is_pending());
    }

    #[test]
    fn signature_ignores_worktree_edits_but_tracks_refs_and_shallow_state() {
        let (dir, _) = TempRepo::with_commit();
        let repo = git2::Repository::open(&dir.0).unwrap();
        let initial = crate::history_signature(&repo);
        std::fs::write(dir.0.join("file.txt"), "working tree change\n").unwrap();
        assert_eq!(crate::history_signature(&repo), initial);

        let head = repo.head().unwrap().peel_to_commit().unwrap();
        repo.branch("extra", &head, false).unwrap();
        let with_ref = crate::history_signature(&repo);
        assert_ne!(with_ref, initial);

        std::fs::write(repo.path().join("shallow"), "fixture-shallow-oid\n").unwrap();
        assert_ne!(crate::history_signature(&repo), with_ref);
    }
}
