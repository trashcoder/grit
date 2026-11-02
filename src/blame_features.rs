use super::*;
use std::{
    fs,
    path::Component,
    str,
    sync::mpsc::{self, Receiver, TryRecvError},
};

const MAX_BLAME_BYTES: usize = 4 * 1024 * 1024;
type BlameData = (String, Vec<BlameLine>, Vec<BlameCommit>);
type BlameResult = Result<BlameData, String>;
type BlameReceiver = Receiver<BlameResult>;

#[derive(Clone, Debug)]
pub(super) struct BlameLine {
    pub final_commit_id: git2::Oid,
    pub author: String,
    pub email: String,
    pub committed_time: Option<i64>,
    pub original_line: usize,
    pub current_line: usize,
    pub text: String,
}

#[derive(Clone, Debug)]
pub(super) struct BlameCommit {
    pub id: git2::Oid,
    pub author: String,
    pub committed_time: Option<i64>,
    pub line_count: usize,
}

#[derive(Default)]
pub(super) struct BlameState {
    pub path: String,
    pub requested_revision: Option<String>,
    pub version: String,
    pub lines: Vec<BlameLine>,
    pub commits: Vec<BlameCommit>,
    pub error: Option<String>,
    pub loading: bool,
    receiver: Option<BlameReceiver>,
}

fn checked_path(path: &str) -> Result<PathBuf, String> {
    file_features::validate_repo_path(path)?;
    Ok(Path::new(path).to_path_buf())
}

fn checked_text(bytes: &[u8]) -> Result<&str, String> {
    if bytes.len() > MAX_BLAME_BYTES {
        return Err("Blame preview is limited to 4 MiB".into());
    }
    if bytes.contains(&0) {
        return Err("Binary files cannot be shown in the blame view".into());
    }
    str::from_utf8(bytes).map_err(|e| format!("File is not valid UTF-8: {e}"))
}

fn current_bytes(root: &Path, relative: &Path) -> Result<Vec<u8>, String> {
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    let mut cursor = root.clone();
    for component in relative.components() {
        let Component::Normal(part) = component else {
            return Err("File path must stay inside the repository".into());
        };
        cursor.push(part);
        let metadata = fs::symlink_metadata(&cursor).map_err(|e| e.to_string())?;
        if metadata.file_type().is_symlink() {
            return Err("Symbolic links are not supported in the blame view".into());
        }
    }
    let canonical = cursor.canonicalize().map_err(|e| e.to_string())?;
    if !canonical.starts_with(&root) {
        return Err("File path resolves outside the repository".into());
    }
    let metadata = fs::metadata(&canonical).map_err(|e| e.to_string())?;
    if !metadata.is_file() {
        return Err("Blame requires a regular file".into());
    }
    if metadata.len() > MAX_BLAME_BYTES as u64 {
        return Err("Blame preview is limited to 4 MiB".into());
    }
    fs::read(canonical).map_err(|e| e.to_string())
}

fn blame_data(repo: &Repository, path: &str, revision: Option<&str>) -> BlameResult {
    let relative = checked_path(path)?;
    let root = repo
        .workdir()
        .ok_or("Blame requires a repository with a worktree")?;
    let (content, base, version, historical) = if let Some(revision) = revision {
        let object = repo
            .revparse_single(revision)
            .map_err(|e| format!("Invalid revision: {e}"))?;
        let commit = object
            .peel_to_commit()
            .map_err(|e| format!("Revision is not a commit: {e}"))?;
        let tree = commit.tree().map_err(|e| e.to_string())?;
        let entry = tree
            .get_path(&relative)
            .map_err(|e| format!("File is absent at this revision: {e}"))?;
        if entry.filemode() == 0o120000 {
            return Err("Symbolic links are not supported in the blame view".into());
        }
        let blob = repo.find_blob(entry.id()).map_err(|e| e.to_string())?;
        checked_text(blob.content())?;
        (
            blob.content().to_vec(),
            commit.id(),
            format!("Revision {} ({})", &commit.id().to_string()[..8], revision),
            true,
        )
    } else {
        let content = current_bytes(root, &relative)?;
        checked_text(&content)?;
        let head = repo
            .head()
            .and_then(|head| head.peel_to_commit())
            .map_err(|e| format!("Could not resolve HEAD: {e}"))?;
        (
            content,
            head.id(),
            "Working copy (compared with HEAD)".into(),
            false,
        )
    };

    let mut options = git2::BlameOptions::new();
    options.newest_commit(base);
    let original_blame = repo
        .blame_file(&relative, Some(&mut options))
        .map_err(|e| format!("Could not blame file: {e}"))?;
    let blamed = if historical {
        None
    } else {
        Some(
            original_blame
                .blame_buffer(&content)
                .map_err(|e| format!("Could not blame working copy: {e}"))?,
        )
    };
    let blame = blamed.as_ref().unwrap_or(&original_blame);
    let text = checked_text(&content)?;
    let raw_lines: Vec<&str> = text.lines().collect();
    let mut lines = Vec::with_capacity(raw_lines.len());
    for (index, line) in raw_lines.iter().enumerate() {
        let current_line = index + 1;
        let hunk = blame
            .get_line(current_line)
            .ok_or_else(|| format!("No blame data for line {current_line}"))?;
        let id = hunk.final_commit_id();
        let signature = hunk.final_signature();
        let (author, email, committed_time) = if id.is_zero() {
            ("Uncommitted changes".into(), String::new(), None)
        } else {
            (
                signature.name().unwrap_or("Unknown author").to_owned(),
                signature.email().unwrap_or("").to_owned(),
                Some(signature.when().seconds()),
            )
        };
        let original_line = if id.is_zero() {
            0
        } else {
            hunk.orig_start_line() + current_line - hunk.final_start_line()
        };
        lines.push(BlameLine {
            final_commit_id: id,
            author,
            email,
            committed_time,
            original_line,
            current_line,
            text: (*line).to_owned(),
        });
    }
    let mut commits: Vec<BlameCommit> = Vec::new();
    for line in &lines {
        if let Some(existing) = commits
            .iter_mut()
            .find(|commit| commit.id == line.final_commit_id)
        {
            existing.line_count += 1;
        } else {
            commits.push(BlameCommit {
                id: line.final_commit_id,
                author: line.author.clone(),
                committed_time: line.committed_time,
                line_count: 1,
            });
        }
    }
    commits.sort_by_key(|commit| std::cmp::Reverse(commit.committed_time));
    Ok((version, lines, commits))
}

impl GritApp {
    pub(super) fn open_blame(&mut self, path: String, revision: Option<String>) {
        self.blame_state = BlameState {
            path: path.clone(),
            requested_revision: revision.clone(),
            loading: true,
            ..Default::default()
        };
        let Some(repo_path) = self.path.clone() else {
            self.blame_state.loading = false;
            self.blame_state.error = Some("Open a repository first".into());
            self.tab = 4;
            return;
        };
        let (sender, receiver) = mpsc::channel();
        self.blame_state.receiver = Some(receiver);
        std::thread::spawn(move || {
            let result = Repository::open(repo_path)
                .map_err(|e| e.to_string())
                .and_then(|repo| blame_data(&repo, &path, revision.as_deref()));
            let _ = sender.send(result);
        });
        self.tab = 4;
    }

    pub(super) fn blame_workspace(&mut self, ui: &mut egui::Ui) {
        if let Some(receiver) = self.blame_state.receiver.take() {
            match receiver.try_recv() {
                Ok(Ok((version, lines, commits))) => {
                    self.blame_state.loading = false;
                    self.blame_state.version = version;
                    self.blame_state.lines = lines;
                    self.blame_state.commits = commits;
                }
                Ok(Err(error)) => {
                    self.blame_state.loading = false;
                    self.blame_state.error = Some(error);
                }
                Err(TryRecvError::Disconnected) => {
                    self.blame_state.loading = false;
                    self.blame_state.error = Some("Blame worker stopped unexpectedly".into());
                }
                Err(TryRecvError::Empty) => {
                    self.blame_state.receiver = Some(receiver);
                    ui.ctx()
                        .request_repaint_after(std::time::Duration::from_millis(100));
                }
            }
        }
        ui.heading(format!("Blame · {}", self.blame_state.path));
        let mut refresh = false;
        ui.horizontal(|ui| {
            ui.label(RichText::new(&self.blame_state.version).color(MUTED));
            if ui.button("Refresh").clicked() && !self.blame_state.loading {
                refresh = true;
            }
            if self.blame_state.loading {
                ui.spinner();
                ui.label("Loading blame…");
            }
        });
        if refresh {
            let path = self.blame_state.path.clone();
            let revision = self.blame_state.requested_revision.clone();
            self.open_blame(path, revision);
        }
        if let Some(error) = &self.blame_state.error {
            ui.colored_label(Color32::LIGHT_RED, error);
            return;
        }
        if self.blame_state.loading {
            return;
        }
        if self.blame_state.lines.is_empty() {
            ui.label("No lines to annotate.");
            return;
        }
        let mut selected_commit = None;
        egui::SidePanel::left("blame_timeline_panel")
            .resizable(true)
            .default_width(250.)
            .width_range(180. ..=360.)
            .show_inside(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("blame_timeline_scroll")
                    .show(ui, |ui| {
                        ui.strong("Commits in this file");
                        for commit in &self.blame_state.commits {
                            let label = if commit.id.is_zero() {
                                format!("Uncommitted · {} lines", commit.line_count)
                            } else {
                                let date = commit
                                    .committed_time
                                    .and_then(|time| {
                                        chrono::DateTime::<chrono::Utc>::from_timestamp(time, 0)
                                    })
                                    .map(|date| date.format("%Y-%m-%d").to_string())
                                    .unwrap_or_else(|| "Unknown date".into());
                                format!(
                                    "{} · {} lines\n{} · {}",
                                    date,
                                    commit.line_count,
                                    commit.author,
                                    &commit.id.to_string()[..8]
                                )
                            };
                            if ui.selectable_label(false, label).clicked() && !commit.id.is_zero() {
                                selected_commit = Some(commit.id.to_string());
                            }
                        }
                    });
            });
        egui::CentralPanel::default().show_inside(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.;
            ui.spacing_mut().interact_size.y = 22.;
            egui::ScrollArea::both()
                .id_salt("blame_lines_scroll")
                .show_rows(ui, 22., self.blame_state.lines.len(), |ui, visible_rows| {
                    for row in visible_rows {
                        if let Some(line) = self.blame_state.lines.get(row) {
                            ui.horizontal(|ui| {
                                let date = line
                                    .committed_time
                                    .and_then(|time| {
                                        chrono::DateTime::<chrono::Utc>::from_timestamp(time, 0)
                                    })
                                    .map(|date| date.format("%Y-%m-%d").to_string())
                                    .unwrap_or_else(|| "Uncommitted".into());
                                let id = if line.final_commit_id.is_zero() {
                                    "working".into()
                                } else {
                                    line.final_commit_id.to_string()[..8].to_owned()
                                };
                                let response = ui.add_sized(
                                    [360., 22.],
                                    egui::Label::new(
                                        RichText::new(format!(
                                            "{date} {id} {:>5}/{:>5} {}",
                                            line.original_line, line.current_line, line.author
                                        ))
                                        .monospace(),
                                    )
                                    .truncate()
                                    .sense(egui::Sense::click()),
                                );
                                let response = response
                                    .on_hover_text(format!("{} <{}>", line.author, line.email));
                                if !line.final_commit_id.is_zero() && response.clicked() {
                                    selected_commit = Some(line.final_commit_id.to_string());
                                }
                                ui.add(
                                    egui::Label::new(RichText::new(&line.text).monospace())
                                        .extend(),
                                );
                            });
                        }
                    }
                });
        });
        if let Some(id) = selected_commit {
            self.search.clear();
            self.history_options.query_cache = None;
            self.history_options.only_starred = false;
            self.history_options.first_parent = false;
            self.set_history_scope(None);
            self.select_history_commit(id);
            self.tab = 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct TestDir(PathBuf);

    impl TestDir {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "grit-blame-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn commit(
        repo: &Repository,
        path: &Path,
        content: &[u8],
        name: &str,
        email: &str,
        message: &str,
    ) -> git2::Oid {
        fs::write(path, content).unwrap();
        let mut index = repo.index().unwrap();
        index
            .add_path(Path::new(path.file_name().unwrap()))
            .unwrap();
        index.write().unwrap();
        let tree_id = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let signature = git2::Signature::now(name, email).unwrap();
        let parent = repo.head().ok().and_then(|head| head.peel_to_commit().ok());
        let parents: Vec<_> = parent.iter().collect();
        repo.commit(
            Some("HEAD"),
            &signature,
            &signature,
            message,
            &tree,
            &parents,
        )
        .unwrap()
    }

    #[test]
    fn real_blame_tracks_authors_revisions_and_working_changes() {
        let dir = TestDir::new();
        let repo = Repository::init(dir.path()).unwrap();
        let path = dir.path().join("file [x]*.txt");
        let first = commit(
            &repo,
            &path,
            b"alpha\nbeta\n",
            "Ada",
            "ada@example.test",
            "first",
        );
        let second = commit(
            &repo,
            &path,
            b"alpha\nchanged\n",
            "Grace",
            "grace@example.test",
            "second",
        );
        let (version, lines, _) = blame_data(&repo, "file [x]*.txt", None).unwrap();
        assert!(version.starts_with("Working copy"));
        assert_eq!(
            lines
                .iter()
                .map(|line| line.final_commit_id)
                .collect::<Vec<_>>(),
            [first, second]
        );
        assert_eq!(lines[0].original_line, 1);
        assert_eq!(lines[1].current_line, 2);
        let (_, old, _) = blame_data(&repo, "file [x]*.txt", Some(&first.to_string())).unwrap();
        assert_eq!(old.len(), 2);
        assert!(old.iter().all(|line| line.final_commit_id == first));
        fs::write(&path, b"alpha\nchanged\npending\n").unwrap();
        let (_, dirty, commits) = blame_data(&repo, "file [x]*.txt", None).unwrap();
        assert!(dirty[2].final_commit_id.is_zero());
        assert_eq!(dirty[2].author, "Uncommitted changes");
        assert!(
            commits
                .iter()
                .any(|commit| commit.id.is_zero() && commit.line_count == 1)
        );
    }

    #[test]
    fn rejects_untracked_symlink_binary_invalid_utf8_and_oversize_files() {
        let dir = TestDir::new();
        let repo = Repository::init(dir.path()).unwrap();
        let tracked = dir.path().join("tracked");
        commit(
            &repo,
            &tracked,
            b"ok\n",
            "Ada",
            "ada@example.test",
            "initial",
        );
        assert!(blame_data(&repo, "new", None).is_err());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("tracked", dir.path().join("link")).unwrap();
            assert!(
                blame_data(&repo, "link", None)
                    .unwrap_err()
                    .contains("Symbolic links")
            );
        }
        fs::write(&tracked, [0xff, 0xfe]).unwrap();
        assert!(
            blame_data(&repo, "tracked", None)
                .unwrap_err()
                .contains("UTF-8")
        );
        fs::write(&tracked, b"binary\0data").unwrap();
        assert!(
            blame_data(&repo, "tracked", None)
                .unwrap_err()
                .contains("Binary")
        );
        fs::write(&tracked, vec![b'x'; MAX_BLAME_BYTES + 1]).unwrap();
        assert!(
            blame_data(&repo, "tracked", None)
                .unwrap_err()
                .contains("4 MiB")
        );
        assert!(checked_path("../outside").is_err());
    }
}
