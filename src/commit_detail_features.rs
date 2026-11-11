use super::{CommitFile, commit_files, diff_commit_path, diff_preferences};
use git2::Repository;
use std::{
    collections::{HashMap, VecDeque},
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, Sender},
    thread,
    time::{Duration, Instant},
};

const CACHE_LIMIT: usize = 12;
const CACHE_BYTES_LIMIT: usize = 16 * 1024 * 1024;

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub(super) struct DetailKey {
    pub(super) repo: PathBuf,
    pub(super) commit: String,
    pub(super) file: Option<String>,
    context_lines: u32,
    ignore_whitespace: bool,
}

impl DetailKey {
    pub(super) fn matches_preferences(
        &self,
        preferences: &diff_preferences::DiffPreferences,
    ) -> bool {
        self.context_lines == preferences.context_lines
            && self.ignore_whitespace == preferences.ignore_whitespace
    }
}

#[derive(Clone)]
pub(super) struct CommitDetail {
    pub files: Vec<CommitFile>,
    pub diff: Option<Result<String, String>>,
}
impl CommitDetail {
    fn size_bytes(&self) -> usize {
        self.files
            .iter()
            .map(|file| file.path.capacity() + std::mem::size_of::<CommitFile>())
            .sum::<usize>()
            + self.diff.as_ref().map_or(0, |result| match result {
                Ok(value) | Err(value) => value.capacity(),
            })
    }
}

struct Job {
    generation: u64,
    key: DetailKey,
    include_diff: bool,
}
type DetailResult = (u64, DetailKey, Result<CommitDetail, String>);

pub(super) struct CommitDetailLoader {
    job_tx: Sender<Job>,
    rx: Receiver<DetailResult>,
    generation: u64,
    active: Option<(u64, DetailKey)>,
    active_include_diff: bool,
    pending: Option<Job>,
    current: Option<(u64, DetailKey)>,
    current_include_diff: bool,
    visible: Option<(DetailKey, CommitDetail)>,
    failed_current: bool,
    selected_at: Instant,
    cache: HashMap<DetailKey, CommitDetail>,
    lru: VecDeque<DetailKey>,
    cache_bytes: usize,
}

impl Default for CommitDetailLoader {
    fn default() -> Self {
        let (job_tx, job_rx) = mpsc::channel::<Job>();
        let (tx, rx) = mpsc::channel::<DetailResult>();
        thread::spawn(move || {
            while let Ok(job) = job_rx.recv() {
                let result = Repository::open(&job.key.repo)
                    .map_err(|error| error.to_string())
                    .map(|repo| {
                        let files = commit_files(&repo, &job.key.commit);
                        let diff = job.include_diff.then(|| {
                            let preferences = diff_preferences::DiffPreferences {
                                context_lines: job.key.context_lines,
                                ignore_whitespace: job.key.ignore_whitespace,
                            };
                            if preferences == Default::default() {
                                Ok(diff_commit_path(
                                    &repo,
                                    &job.key.commit,
                                    job.key.file.as_deref(),
                                ))
                            } else {
                                diff_preferences::render_commit_diff(
                                    &repo,
                                    &job.key.commit,
                                    job.key.file.as_deref(),
                                    &preferences,
                                )
                            }
                        });
                        CommitDetail { files, diff }
                    });
                if tx.send((job.generation, job.key, result)).is_err() {
                    break;
                }
            }
        });
        Self {
            job_tx,
            rx,
            generation: 0,
            active: None,
            active_include_diff: false,
            pending: None,
            current: None,
            current_include_diff: false,
            visible: None,
            failed_current: false,
            selected_at: Instant::now(),
            cache: HashMap::new(),
            lru: VecDeque::new(),
            cache_bytes: 0,
        }
    }
}

impl CommitDetailLoader {
    pub fn request(
        &mut self,
        repo: PathBuf,
        commit: String,
        file: Option<String>,
        context_lines: u32,
        ignore_whitespace: bool,
        include_diff: bool,
    ) -> bool {
        let key = DetailKey {
            repo,
            commit,
            file,
            context_lines,
            ignore_whitespace,
        };
        let same_key = self.current.as_ref() == Some(&(self.generation, key.clone()));
        if !same_key {
            self.pending = None;
            self.selected_at = Instant::now();
            self.visible = None;
            self.failed_current = false;
            self.current_include_diff = include_diff;
        } else if self.failed_current {
            return false;
        } else if include_diff && !self.current_include_diff {
            self.current_include_diff = true;
        } else {
            return false;
        }
        self.current = Some((self.generation, key.clone()));
        let complete = self
            .cache
            .get(&key)
            .is_some_and(|entry| !include_diff || entry.diff.is_some());
        if complete {
            self.touch(&key);
            self.visible = self
                .cache
                .get(&key)
                .cloned()
                .map(|detail| (key.clone(), detail));
            self.failed_current = false;
            self.pending = None;
            return true;
        }
        let job = Job {
            generation: self.generation,
            key,
            include_diff,
        };
        if self.active.is_none() {
            self.start(job);
        } else if self.active.as_ref() != Some(&(job.generation, job.key.clone()))
            || (job.include_diff && !self.active_include_diff)
        {
            self.pending = Some(job);
        }
        true
    }

    pub fn take_current(&mut self) -> Option<(DetailKey, Result<CommitDetail, String>)> {
        let mut latest = None;
        while let Ok((generation, key, result)) = self.rx.try_recv() {
            if self.active.as_ref() == Some(&(generation, key.clone())) {
                self.active = None;
            }
            if generation == self.generation {
                if result.is_err() && self.current.as_ref() == Some(&(generation, key.clone())) {
                    self.failed_current = true;
                }
                if let Ok(detail) = &result {
                    if self.current.as_ref() == Some(&(generation, key.clone())) {
                        self.visible = Some((key.clone(), detail.clone()));
                    }
                    self.insert(key.clone(), detail.clone());
                }
                if self.current.as_ref() == Some(&(generation, key.clone())) {
                    latest = Some((key, result));
                }
            }
        }
        self.start_pending();
        latest
    }

    pub fn cached(
        &mut self,
        repo: &Path,
        commit: &str,
        file: Option<&str>,
        context_lines: u32,
        ignore_whitespace: bool,
    ) -> Option<CommitDetail> {
        let key = DetailKey {
            repo: repo.to_path_buf(),
            commit: commit.to_owned(),
            file: file.map(str::to_owned),
            context_lines,
            ignore_whitespace,
        };
        if let Some((visible_key, visible)) = &self.visible
            && visible_key == &key
        {
            return Some(visible.clone());
        }
        let value = self.cache.get(&key).cloned()?;
        self.touch(&key);
        Some(value)
    }

    pub fn clear_selection(&mut self) {
        self.current = None;
        self.current_include_diff = false;
        self.visible = None;
        self.failed_current = false;
        self.pending = None;
    }

    pub fn reset(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.clear_selection();
        self.cache.clear();
        self.lru.clear();
        self.cache_bytes = 0;
    }

    pub fn current_is_pending(&self) -> bool {
        self.current.as_ref().is_some_and(|(generation, key)| {
            *generation == self.generation
                && (self.active.as_ref() == Some(&(*generation, key.clone()))
                    || self.pending.as_ref().is_some_and(|job| job.key == *key))
        })
    }

    pub fn is_current_selection(
        &self,
        repo: &Path,
        commit: &str,
        file: Option<&str>,
        context_lines: u32,
        ignore_whitespace: bool,
    ) -> bool {
        self.current.as_ref().is_some_and(|(generation, key)| {
            *generation == self.generation
                && key.repo == *repo
                && key.commit == commit
                && key.file.as_deref() == file
                && key.context_lines == context_lines
                && key.ignore_whitespace == ignore_whitespace
        })
    }

    pub fn diff_debounce_ready(&self) -> bool {
        self.selected_at.elapsed() >= Duration::from_millis(140)
    }

    fn start(&mut self, job: Job) {
        self.active_include_diff = job.include_diff;
        self.active = Some((job.generation, job.key.clone()));
        if self.job_tx.send(job).is_err() {
            self.active = None;
        }
    }
    fn start_pending(&mut self) {
        if self.active.is_none()
            && let Some(job) = self.pending.take()
        {
            self.start(job);
        }
    }
    fn touch(&mut self, key: &DetailKey) {
        self.lru.retain(|entry| entry != key);
        self.lru.push_back(key.clone());
    }
    fn insert(&mut self, key: DetailKey, value: CommitDetail) {
        let size = cache_size(&key, &value);
        if size > CACHE_BYTES_LIMIT {
            return;
        }
        if let Some(old) = self.cache.remove(&key) {
            self.cache_bytes -= cache_size(&key, &old);
        }
        self.lru.retain(|entry| entry != &key);
        self.cache_bytes += size;
        self.cache.insert(key.clone(), value);
        self.lru.push_back(key);
        while self.lru.len() > CACHE_LIMIT || self.cache_bytes > CACHE_BYTES_LIMIT {
            let Some(old_key) = self.lru.pop_front() else {
                break;
            };
            if let Some(old) = self.cache.remove(&old_key) {
                self.cache_bytes -= cache_size(&old_key, &old);
            }
        }
    }
}

fn cache_size(key: &DetailKey, value: &CommitDetail) -> usize {
    value.size_bytes()
        + key.repo.as_os_str().len()
        + key.commit.len()
        + key.file.as_ref().map_or(0, String::len)
        + std::mem::size_of::<DetailKey>()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unchanged_selection_is_not_requeued_and_latest_pending_wins() {
        let mut loader = CommitDetailLoader::default();
        let repo = PathBuf::from("/definitely/missing/grit-test-repository");
        assert!(loader.request(repo.clone(), "first".into(), None, 3, false, false));
        assert!(!loader.request(repo.clone(), "first".into(), None, 3, false, false));
        assert!(loader.request(repo.clone(), "second".into(), None, 3, false, false));
        assert!(loader.request(repo.clone(), "latest".into(), None, 3, false, false));
        assert_eq!(
            loader.pending.as_ref().map(|job| job.key.commit.as_str()),
            Some("latest")
        );
        loader.reset();
        assert!(loader.pending.is_none());
        assert!(loader.current.is_none());
    }

    #[test]
    fn diff_request_upgrades_files_stage_once() {
        let mut loader = CommitDetailLoader::default();
        let repo = PathBuf::from("/definitely/missing/grit-test-repository");
        assert!(loader.request(repo.clone(), "commit".into(), None, 3, false, false));
        assert!(!loader.request(repo.clone(), "commit".into(), None, 3, false, false));
        assert!(loader.request(repo.clone(), "commit".into(), None, 3, false, true));
        assert!(!loader.request(repo, "commit".into(), None, 3, false, true));
    }
}
