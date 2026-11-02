#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use eframe::egui::{self, Align, Color32, FontId, Layout, RichText, Stroke};
use git2::{BranchType, Repository, Status, StatusOptions};
use std::{
    collections::{HashMap, HashSet},
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc::{self, Receiver},
};

const BG: Color32 = Color32::from_rgb(25, 28, 34);
const PANEL: Color32 = Color32::from_rgb(29, 33, 40);
const PANEL2: Color32 = Color32::from_rgb(35, 39, 47);
const BORDER: Color32 = Color32::from_rgb(48, 55, 67);
const TEXT: Color32 = Color32::from_rgb(226, 231, 239);
const MUTED: Color32 = Color32::from_rgb(146, 156, 172);
const BLUE: Color32 = Color32::from_rgb(98, 164, 255);
const GREEN: Color32 = Color32::from_rgb(107, 202, 157);

#[derive(Clone)]
struct Change {
    path: String,
    staged: bool,
    status: &'static str,
}
#[derive(Clone, Default)]
struct Commit {
    id: String,
    parents: Vec<String>,
    short: String,
    message: String,
    author: String,
    author_email: String,
    full_message: String,
    committer: String,
    timestamp: i64,
    time_offset: i32,
    date: String,
    refs: Vec<String>,
}
#[derive(Clone)]
struct CommitFile {
    path: String,
    status: &'static str,
}
#[derive(Default)]
struct GritApp {
    integrations: integration_features::IntegrationTools,
    history_options: history_features::HistoryOptions,
    focus_mode: bool,
    focused_history_table: bool,
    was_focused: bool,
    wrap_diff: bool,
    load_large_diff: bool,
    selected_diff_lines: HashSet<usize>,
    show_commit_options: bool,
    operation_log: Vec<String>,
    show_operation_log: bool,
    repo_tools: repository_features::RepositoryTools,
    file_tools: file_features::FileTools,
    commit_options: commit_features::CommitOptions,
    path: Option<PathBuf>,
    open_repos: Vec<PathBuf>,
    recent_repos: Vec<PathBuf>,
    commits: Vec<Commit>,
    history_error: Option<String>,
    history_scope: Option<(String, HashSet<String>)>,
    changes: Vec<Change>,
    stashes: Vec<(usize, String)>,
    tracked_files: Vec<String>,
    branches: Vec<String>,
    remote_branches: Vec<String>,
    tags: Vec<String>,
    branch: String,
    branch_has_upstream: bool,
    push_remote: Option<String>,
    selected_commit: Option<String>,
    selected_commit_file: Option<String>,
    commit_files: Vec<CommitFile>,
    selected_file: Option<String>,
    selected_staged: bool,
    file_content: String,
    diff: String,
    message: String,
    search: String,
    file_search: String,
    notice: String,
    tab: usize,
    clone_url: String,
    clone_path: String,
    new_branch: String,
    new_tag: String,
    show_clone: bool,
    remote_name: String,
    remote_url: String,
    amend: bool,
    operation: Option<(String, Receiver<Result<String, String>>)>,
    open_after_operation: Option<PathBuf>,
    aux_output: String,
    pending_discard: Option<String>,
    pending_stash_drop: Option<usize>,
    pending_init: Option<PathBuf>,
    rebase_draft: Option<(String, String)>,
    conflict_draft: Option<(String, String)>,
    patch_draft: Option<(String, String)>,
}
impl GritApp {
    fn repo(&self) -> Option<Repository> {
        self.path.as_ref().and_then(|p| Repository::open(p).ok())
    }
    fn remember_repo(&mut self, path: &Path) {
        let path = path.to_path_buf();
        self.recent_repos.retain(|recent| recent != &path);
        self.recent_repos.insert(0, path);
        self.recent_repos.truncate(8);
    }
    fn open(&mut self, p: PathBuf) {
        if self.operation.is_some() {
            return;
        }
        self.pending_init = None;
        match Repository::open(&p) {
            Ok(r) => {
                let opened_path = r.workdir().unwrap_or(&p).to_path_buf();
                if self.path.as_ref() != Some(&opened_path) {
                    self.reset_repo_selection();
                }
                self.path = Some(opened_path);
                if let Some(path) = self.path.clone() {
                    if !self.open_repos.contains(&path) {
                        self.open_repos.push(path.clone());
                    }
                    self.remember_repo(&path)
                }
                self.tab = 1;
                self.notice = "Repository loaded".into();
                if self.message.is_empty() {
                    match commit_features::load_template(&r) {
                        Ok(Some(template)) => self.message = template,
                        Ok(None) => self.message = self.commit_options.template.clone(),
                        Err(error) => self.notice = error,
                    }
                }
                self.refresh();
            }
            Err(error) if error.code() == git2::ErrorCode::NotFound => {
                self.notice = "This folder is not a Git repository".into();
                self.pending_init = Some(p);
            }
            Err(e) => self.notice = format!("Could not open repository: {e}"),
        }
    }
    fn init_repository(&mut self, p: PathBuf) {
        if self.operation.is_some() {
            return;
        }
        match Repository::init(&p) {
            Ok(repo) => {
                let opened_path = repo.workdir().unwrap_or(&p).to_path_buf();
                if self.path.as_ref() != Some(&opened_path) {
                    self.reset_repo_selection();
                }
                self.path = Some(opened_path);
                if let Some(path) = self.path.clone() {
                    if !self.open_repos.contains(&path) {
                        self.open_repos.push(path.clone());
                    }
                    self.remember_repo(&path)
                }
                self.tab = 1;
                self.notice = "Repository initialized".into();
                self.refresh();
            }
            Err(e) => self.notice = format!("Could not initialize repository: {e}"),
        }
    }
    fn reset_repo_selection(&mut self) {
        self.selected_commit = None;
        self.repo_tools = Default::default();
        self.file_tools = Default::default();
        self.selected_diff_lines.clear();
        self.history_options.navigation.clear();
        self.history_options.navigation_index = 0;
        self.history_scope = None;
        self.selected_commit_file = None;
        self.load_large_diff = false;
        self.commit_files.clear();
        self.selected_file = None;
        self.file_content.clear();
        self.diff.clear();
        self.aux_output.clear();
        self.conflict_draft = None;
        self.patch_draft = None;
        self.pending_discard = None;
        self.pending_stash_drop = None;
        self.pending_init = None;
        self.rebase_draft = None;
        self.branch_has_upstream = false;
        self.push_remote = None;
    }
    fn close_repo_tab(&mut self, path: &Path) {
        if self.operation.is_some() {
            return;
        }
        let was_active = self.path.as_deref() == Some(path);
        self.open_repos.retain(|open| open != path);
        if was_active {
            self.path = None;
            self.reset_repo_selection();
            self.changes.clear();
            self.commits.clear();
            self.stashes.clear();
            self.tracked_files.clear();
            self.branches.clear();
            self.remote_branches.clear();
            self.tags.clear();
            self.branch.clear();
            self.branch_has_upstream = false;
            self.push_remote = None;
            if let Some(next) = self.open_repos.last().cloned() {
                self.open(next);
            }
        }
    }
    fn git_action(&mut self, label: &str, args: &[&str]) {
        self.start_git_command(label, args.iter().map(|s| (*s).to_owned()).collect());
    }
    fn start_git_command(&mut self, label: &str, args: Vec<String>) {
        if self.operation.is_some() {
            return;
        }
        let Some(path) = self.path.clone() else {
            self.notice = "Open a repository first".into();
            return;
        };
        let label = label.to_owned();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let result = Command::new("git")
                .arg("-C")
                .arg(path)
                .args(&args)
                .output()
                .map_err(|e| e.to_string())
                .and_then(|o| {
                    if o.status.success() {
                        Ok(String::from_utf8_lossy(&o.stdout).trim().to_owned())
                    } else {
                        Err(String::from_utf8_lossy(&o.stderr).trim().to_owned())
                    }
                });
            let _ = tx.send(result);
        });
        self.notice = format!("{label} in progress…");
        self.operation = Some((label, rx));
    }
    fn prepare_interactive_rebase(&mut self, upstream: &str) {
        if self.operation.is_some() {
            return;
        }
        let Some(repo) = self.repo() else {
            self.notice = "Open a repository first".into();
            return;
        };
        if !self.changes.is_empty() {
            self.notice = "Commit or stash working tree changes before rebasing".into();
            return;
        }
        let Ok(branch) = repo.find_branch(upstream, BranchType::Local) else {
            self.notice = format!("Local branch not found: {upstream}");
            return;
        };
        let Some(upstream_id) = branch.get().target() else {
            self.notice = format!("Branch has no commit: {upstream}");
            return;
        };
        let mut walk = match repo.revwalk() {
            Ok(walk) => walk,
            Err(error) => {
                self.notice = format!("Could not read commit history: {error}");
                return;
            }
        };
        if let Err(error) = walk
            .push_head()
            .and_then(|()| walk.hide(upstream_id))
            .and_then(|()| walk.set_sorting(git2::Sort::TOPOLOGICAL | git2::Sort::REVERSE))
        {
            self.notice = format!("Could not prepare rebase plan: {error}");
            return;
        }
        let mut lines = Vec::new();
        for result in walk {
            let Ok(id) = result else {
                self.notice = "Could not read all commits for the rebase plan".into();
                return;
            };
            let Ok(commit) = repo.find_commit(id) else {
                self.notice = format!("Could not read commit {id}");
                return;
            };
            if commit.parent_count() > 1 {
                self.notice = format!(
                    "Interactive rebase cannot preserve merge commit {} yet",
                    &id.to_string()[..7]
                );
                return;
            }
            let summary = commit.summary().unwrap_or("(no commit message)");
            lines.push(format!("pick {id} {summary}"));
        }
        if lines.is_empty() {
            self.notice = format!("There are no commits to rebase onto {upstream}");
            return;
        }
        self.rebase_draft = Some((upstream.to_owned(), lines.join("\n")));
    }
    fn start_interactive_rebase(&mut self, upstream: &str, todo: &str) {
        if self.operation.is_some() {
            return;
        }
        let Some(path) = self.path.clone() else {
            self.notice = "Open a repository first".into();
            return;
        };
        if !self.changes.is_empty() {
            self.notice = "Commit or stash working tree changes before rebasing".into();
            return;
        }
        let Ok(executable) = std::env::current_exe() else {
            self.notice = "Could not locate the grit application for the rebase editor".into();
            return;
        };
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        let draft_path =
            std::env::temp_dir().join(format!("grit-rebase-{}-{stamp}.todo", std::process::id()));
        if let Err(error) = std::fs::write(&draft_path, todo) {
            self.notice = format!("Could not save rebase plan: {error}");
            return;
        }
        let editor = format!(
            "{} --sequence-editor",
            shell_quote(&executable.to_string_lossy())
        );
        let draft_path_for_thread = draft_path.clone();
        let upstream = upstream.to_owned();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let result = Command::new("git")
                .arg("-C")
                .arg(path)
                .args([
                    "-c",
                    "core.editor=true",
                    "rebase",
                    "--interactive",
                    &upstream,
                ])
                .env("GIT_SEQUENCE_EDITOR", editor)
                .env("GRIT_SEQUENCE_DRAFT", draft_path_for_thread)
                .output()
                .map_err(|error| error.to_string())
                .and_then(|output| {
                    let message = String::from_utf8_lossy(&output.stderr).trim().to_owned();
                    if output.status.success() {
                        Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
                    } else {
                        Err(if message.is_empty() {
                            format!("Git exited with {}", output.status)
                        } else {
                            message
                        })
                    }
                });
            let _ = std::fs::remove_file(draft_path);
            let _ = tx.send(result);
        });
        self.operation = Some(("Interactive rebase".into(), rx));
        self.rebase_draft = None;
        self.notice = "Interactive rebase in progress…".into();
    }
    fn clone_repo(&mut self) {
        let url = self.clone_url.trim().to_owned();
        let dest = self.clone_path.trim().to_owned();
        if url.is_empty() || dest.is_empty() {
            self.notice = "Enter a remote URL and destination folder".into();
            return;
        }
        if self.operation.is_some() {
            return;
        }
        let dest_path = PathBuf::from(&dest);
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let result = Command::new("git")
                .args(["clone", &url])
                .arg(&dest_path)
                .output()
                .map_err(|e| e.to_string())
                .and_then(|o| {
                    if o.status.success() {
                        Ok(String::from_utf8_lossy(&o.stdout).trim().to_owned())
                    } else {
                        Err(String::from_utf8_lossy(&o.stderr).trim().to_owned())
                    }
                });
            let _ = tx.send(result);
        });
        self.open_after_operation = Some(PathBuf::from(&dest));
        self.operation = Some(("Clone".into(), rx));
        self.show_clone = false;
        self.notice = "Clone in progress…".into();
    }
    fn refresh(&mut self) {
        let Some(mut repo) = self.repo() else { return };
        self.history_options.query_cache = None;
        self.branch = repo
            .head()
            .ok()
            .and_then(|h| h.shorthand().map(str::to_owned))
            .unwrap_or_else(|| "(detached HEAD)".into());
        self.branch_has_upstream = repo
            .find_branch(&self.branch, BranchType::Local)
            .is_ok_and(|branch| branch.upstream().is_ok());
        let remotes: Vec<String> = repo
            .remotes()
            .ok()
            .into_iter()
            .flat_map(|names| {
                names
                    .iter()
                    .flatten()
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            })
            .collect();
        self.push_remote = remotes
            .iter()
            .find(|name| name.as_str() == "origin")
            .cloned()
            .or_else(|| remotes.first().cloned());
        self.branches = repo
            .branches(Some(BranchType::Local))
            .ok()
            .into_iter()
            .flatten()
            .filter_map(|item| item.ok())
            .filter_map(|(b, _)| b.name().ok().flatten().map(str::to_owned))
            .collect();
        self.remote_branches = repo
            .branches(Some(BranchType::Remote))
            .ok()
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .filter_map(|(branch, _)| branch.name().ok().flatten().map(str::to_owned))
            .filter(|name| !name.ends_with("/HEAD"))
            .collect();
        self.tags = repo
            .references_glob("refs/tags/*")
            .ok()
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .filter_map(|reference| reference.shorthand().map(str::to_owned))
            .collect();
        self.changes = statuses(&repo);
        match history(&repo) {
            Ok(commits) => {
                self.commits = commits;
                self.history_error = None;
            }
            Err(error) => {
                self.commits.clear();
                self.history_error = Some(error);
            }
        }
        if let Some((name, _)) = self.history_scope.clone() {
            self.set_history_scope(Some(name));
        }
        self.tracked_files = repo
            .index()
            .map(|index| {
                index
                    .iter()
                    .filter_map(|entry| String::from_utf8(entry.path).ok())
                    .collect()
            })
            .unwrap_or_default();
        self.stashes.clear();
        let _ = repo.stash_foreach(|index, message, _| {
            self.stashes.push((index, message.to_owned()));
            true
        });
        if let Some(id) = self.selected_commit.clone() {
            self.commit_files = commit_files(&repo, &id);
            if self
                .selected_commit_file
                .as_ref()
                .is_some_and(|path| !self.commit_files.iter().any(|file| &file.path == path))
            {
                self.selected_commit_file = None;
            }
            self.diff = diff_commit_path(&repo, &id, self.selected_commit_file.as_deref())
        } else if let Some(change) = self
            .changes
            .iter()
            .find(|c| {
                self.selected_file.as_deref() == Some(&c.path) && c.staged == self.selected_staged
            })
            .or_else(|| {
                self.changes
                    .iter()
                    .find(|c| c.staged == self.selected_staged)
            })
            .or_else(|| self.changes.first())
            .cloned()
        {
            self.commit_files.clear();
            self.selected_commit_file = None;
            self.selected_staged = change.staged;
            self.select_file(change.path)
        } else {
            self.selected_file = None;
            self.commit_files.clear();
            self.selected_commit_file = None;
            self.diff.clear()
        }
    }
    fn select_file(&mut self, p: String) {
        self.selected_file = Some(p.clone());
        self.selected_commit = None;
        if let Some(r) = self.repo() {
            let patch = diff_file_side(&r, &p, self.selected_staged);
            if self.diff != patch {
                self.selected_diff_lines.clear();
                self.load_large_diff = false;
            }
            self.diff = patch;
        }
    }
    fn preview_file(&mut self, path: &str) {
        self.selected_file = Some(path.to_owned());
        let Some(full_path) = self.path.as_ref().map(|root| root.join(path)) else {
            return;
        };
        self.file_content = match std::fs::symlink_metadata(&full_path) {
            Ok(meta) if meta.file_type().is_symlink() => std::fs::read_link(&full_path)
                .map(|target| format!("Symlink → {}", target.display()))
                .unwrap_or_else(|e| format!("Cannot read symlink: {e}")),
            Ok(meta) if meta.len() > 4 * 1024 * 1024 => format!(
                "Preview skipped: file is {} MB (limit: 4 MB).",
                meta.len() / 1024 / 1024
            ),
            Ok(_) => std::fs::read_to_string(&full_path)
                .unwrap_or_else(|_| "Binary or non UTF-8 file; text preview unavailable.".into()),
            Err(e) => format!("Cannot read file: {e}"),
        };
    }
    fn stage(&mut self, p: &str, unstage: bool) {
        if self.operation.is_some() {
            return;
        }
        if let Some(r) = self.repo()
            && let Ok(mut i) = r.index()
        {
            let result = if unstage {
                let status = r.status_file(Path::new(p)).unwrap_or(Status::CURRENT);
                let result = if status.contains(Status::INDEX_NEW) {
                    i.remove_path(Path::new(p)).and_then(|_| i.write())
                } else {
                    drop(i);
                    r.head()
                        .and_then(|h| h.peel_to_commit())
                        .and_then(|c| r.reset_default(Some(&c.into_object()), [Path::new(p)]))
                };
                match result {
                    Ok(()) => {
                        self.notice = "File unstaged".into();
                        self.refresh();
                    }
                    Err(e) => self.notice = e.to_string(),
                }
                return;
            } else {
                if r.status_file(Path::new(p))
                    .is_ok_and(|status| status.contains(Status::WT_DELETED))
                {
                    i.remove_path(Path::new(p))
                } else {
                    i.add_path(Path::new(p))
                }
            };
            match result.and_then(|_| i.write()) {
                Ok(()) => {
                    self.notice = if unstage {
                        "File unstaged"
                    } else {
                        "File staged"
                    }
                    .into()
                }
                Err(e) => self.notice = e.to_string(),
            }
            self.refresh()
        }
    }
    fn discard(&mut self, path: &str) {
        let Some(repo) = self.repo() else { return };
        let status = repo.status_file(Path::new(path)).unwrap_or(Status::CURRENT);
        let index_bits = Status::INDEX_NEW
            | Status::INDEX_MODIFIED
            | Status::INDEX_DELETED
            | Status::INDEX_RENAMED
            | Status::INDEX_TYPECHANGE;
        if status.contains(Status::WT_NEW) && !status.intersects(index_bits) {
            self.git_action("Discard untracked file", &["clean", "-f", "--", path]);
        } else {
            self.git_action("Discard changes", &["restore", "--worktree", "--", path]);
        }
    }
    fn stage_hunk(&mut self, path: &str, hunk_index: usize) {
        if self.operation.is_some() {
            return;
        }
        let Some(repo) = self.repo() else { return };
        let mut options = git2::DiffOptions::new();
        options
            .pathspec(path)
            .disable_pathspec_match(true)
            .include_untracked(true)
            .recurse_untracked_dirs(true);
        let diff = match repo.diff_index_to_workdir(None, Some(&mut options)) {
            Ok(diff) => diff,
            Err(error) => {
                self.notice = format!("Could not read diff: {error}");
                return;
            }
        };
        if repo
            .status_file(Path::new(path))
            .is_ok_and(|status| status.contains(Status::WT_NEW))
        {
            if let Ok(mut index) = repo.index() {
                match index.add_path(Path::new(path)).and_then(|_| index.write()) {
                    Ok(()) => {
                        self.notice = "New file staged".into();
                        self.refresh()
                    }
                    Err(error) => self.notice = format!("Could not stage file: {error}"),
                }
            }
            return;
        }
        let mut remaining = hunk_index;
        let mut apply_options = git2::ApplyOptions::new();
        apply_options.hunk_callback(|hunk| {
            let selected = hunk.is_some() && remaining == 0;
            if hunk.is_some() && !selected {
                remaining -= 1;
            }
            selected
        });
        match repo.apply(&diff, git2::ApplyLocation::Index, Some(&mut apply_options)) {
            Ok(()) => {
                self.notice = "Diff hunk staged".into();
                self.refresh()
            }
            Err(error) => self.notice = format!("Could not stage hunk: {error}"),
        }
    }
    fn resolve_conflict(&mut self, path: &str, side: Option<&str>) {
        if self.operation.is_some() {
            return;
        }
        let Some(root) = self.path.clone() else {
            return;
        };
        let path = path.to_owned();
        let side = side.map(str::to_owned);
        let label = match side.as_deref() {
            Some("--ours") => "Use our version",
            Some("--theirs") => "Use their version",
            _ => "Mark conflict resolved",
        }
        .to_owned();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let run = |args: &[&str]| {
                Command::new("git")
                    .arg("-C")
                    .arg(&root)
                    .args(args)
                    .output()
                    .map_err(|e| e.to_string())
                    .and_then(|output| {
                        if output.status.success() {
                            Ok(String::new())
                        } else {
                            Err(String::from_utf8_lossy(&output.stderr).trim().to_owned())
                        }
                    })
            };
            let result = if let Some(side) = side.as_deref() {
                run(&["checkout", side, "--", &path]).and_then(|_| run(&["add", "--", &path]))
            } else {
                run(&["add", "--", &path])
            };
            let _ = tx.send(result);
        });
        self.notice = format!("{label} in progress…");
        self.operation = Some((label, rx));
    }
    fn edit_conflict(&mut self, path: &str) {
        let Some(root) = self.path.as_ref() else {
            return;
        };
        let full_path = root.join(path);
        let content = match std::fs::metadata(&full_path) {
            Ok(meta) if meta.len() > 4 * 1024 * 1024 => {
                self.notice = "Conflict editor supports files up to 4 MB".into();
                return;
            }
            Ok(_) => match std::fs::read_to_string(&full_path) {
                Ok(content) => content,
                Err(_) => {
                    self.notice = "Conflict file is binary or not valid UTF-8".into();
                    return;
                }
            },
            Err(error) => {
                self.notice = format!("Could not open conflict file: {error}");
                return;
            }
        };
        self.selected_file = Some(path.to_owned());
        self.conflict_draft = Some((path.to_owned(), content));
    }
    fn edit_stage_patch(&mut self, path: &str) {
        let Some(repo) = self.repo() else { return };
        match unstaged_patch(&repo, path) {
            Ok(patch) if !patch.is_empty() => {
                self.selected_file = Some(path.to_owned());
                self.patch_draft = Some((path.to_owned(), patch));
            }
            Ok(_) => self.notice = "No unstaged changes for this file".into(),
            Err(error) => self.notice = format!("Could not prepare patch: {error}"),
        }
    }
    fn apply_selected_lines(&mut self) {
        let Some(path) = self.selected_file.clone() else {
            return;
        };
        let Some(repo) = self.repo() else {
            return;
        };
        if diff_file_side(&repo, &path, self.selected_staged) != self.diff {
            self.selected_diff_lines.clear();
            self.select_file(path);
            self.notice = "File changed since preview; select lines again".into();
            return;
        }
        match diff_features::selected_patch(
            &self.diff,
            &self.selected_diff_lines,
            self.selected_staged,
        ) {
            Ok(patch) => self.apply_index_patch(
                &patch,
                if self.selected_staged {
                    "Unstage selected lines"
                } else {
                    "Stage selected lines"
                },
            ),
            Err(error) => self.notice = error,
        }
    }
    fn apply_stage_patch(&mut self, patch: &str) {
        self.apply_index_patch(patch, "Stage edited patch");
    }
    fn apply_index_patch(&mut self, patch: &str, label: &str) {
        if self.operation.is_some() {
            return;
        }
        if !patch_has_changes(patch) {
            self.notice = "The patch contains no changed lines".into();
            return;
        }
        let Some(root) = self.path.clone() else {
            return;
        };
        let patch = patch.to_owned();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let result = (|| {
                let mut child = Command::new("git")
                    .arg("-C")
                    .arg(root)
                    .args(["apply", "--cached", "--recount", "--unidiff-zero", "-"])
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn()
                    .map_err(|error| error.to_string())?;
                child
                    .stdin
                    .take()
                    .ok_or_else(|| "Could not open Git input".to_owned())?
                    .write_all(patch.as_bytes())
                    .map_err(|error| error.to_string())?;
                let output = child
                    .wait_with_output()
                    .map_err(|error| error.to_string())?;
                if output.status.success() {
                    Ok(String::new())
                } else {
                    Err(String::from_utf8_lossy(&output.stderr).trim().to_owned())
                }
            })();
            let _ = tx.send(result);
        });
        self.notice = format!("{label} in progress…");
        self.operation = Some((label.into(), rx));
    }
    fn commit(&mut self) {
        let msg = self.message.trim().to_owned();
        if msg.is_empty() {
            self.notice = "Enter a commit message first".into();
            return;
        }
        match commit_features::commit_arguments(&msg, self.amend, &self.commit_options) {
            Ok(args) => self.start_git_command("Commit", args),
            Err(error) => self.notice = error,
        }
    }
    fn checkout(&mut self, name: &str) {
        if self.operation.is_some() {
            return;
        }
        if let Some(r) = self.repo() {
            let result = r.find_branch(name, BranchType::Local).and_then(|b| {
                let obj = b.get().peel(git2::ObjectType::Commit)?;
                r.checkout_tree(&obj, None)?;
                r.set_head(b.get().name().unwrap_or("HEAD"))
            });
            match result {
                Ok(()) => {
                    self.selected_commit = None;
                    self.notice = format!("Switched to {name}");
                    self.refresh()
                }
                Err(e) => self.notice = format!("Could not switch branch: {e}"),
            }
        }
    }
    fn header(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("grit").size(22.).strong().color(TEXT));
            ui.add_space(18.);
            ui.menu_button("Repository", |ui| {
                if ui.button("Open…                 Ctrl+O").clicked() {
                    ui.close_menu();
                    if let Some(path) = rfd::FileDialog::new().pick_folder() {
                        self.open(path);
                    }
                }
                if ui.button("Clone…").clicked() {
                    self.show_clone = true;
                    ui.close_menu();
                }
                if ui.button("Initialize…").clicked() {
                    ui.close_menu();
                    if let Some(path) = rfd::FileDialog::new().pick_folder() {
                        self.init_repository(path);
                    }
                }
                ui.separator();
                for path in self.recent_repos.clone() {
                    if ui.button(path.display().to_string()).clicked() {
                        self.open(path);
                        ui.close_menu();
                    }
                }
            });
            ui.separator();
            if ui
                .small_button(if self.focus_mode {
                    "Restore panes"
                } else {
                    "Focus"
                })
                .on_hover_text("Ctrl/Cmd+M")
                .clicked()
            {
                self.focus_mode = !self.focus_mode;
            }
            if ui.small_button("Log").clicked() {
                self.show_operation_log = true;
            }
            let connected = self.path.is_some() && self.operation.is_none();
            for (label, args) in [
                ("Fetch", vec!["fetch", "--all", "--prune"]),
                ("Pull", vec!["pull", "--ff-only"]),
            ] {
                if ui
                    .add_enabled(connected, egui::Button::new(label))
                    .clicked()
                {
                    self.git_action(label, &args);
                }
            }
            let can_push = self.branch_has_upstream
                || (self.branch != "(detached HEAD)" && self.push_remote.is_some());
            if ui
                .add_enabled(
                    connected && can_push,
                    egui::Button::new(if self.branch_has_upstream {
                        "Push"
                    } else {
                        "Publish"
                    }),
                )
                .clicked()
            {
                if self.branch_has_upstream {
                    self.git_action("Push", &["push"]);
                } else if let Some(remote) = self.push_remote.clone() {
                    self.git_action(
                        "Publish branch",
                        &["push", "--set-upstream", &remote, "HEAD"],
                    );
                }
            }
            ui.separator();
            if ui
                .add_enabled(connected, egui::Button::new("Refresh"))
                .on_hover_text("Ctrl+R")
                .clicked()
            {
                self.refresh();
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(RichText::new(&self.branch).color(GREEN).monospace());
                if self.operation.is_some() {
                    ui.spinner();
                }
            });
        });
        if self.show_clone {
            egui::Window::new("Clone repository")
                .collapsible(false)
                .resizable(false)
                .show(ui.ctx(), |ui| {
                    ui.label("Remote URL");
                    ui.text_edit_singleline(&mut self.clone_url);
                    ui.label("Destination folder");
                    ui.horizontal(|ui| {
                        ui.text_edit_singleline(&mut self.clone_path);
                        if ui.button("Browse…").clicked()
                            && let Some(p) = rfd::FileDialog::new().save_file()
                        {
                            self.clone_path = p.display().to_string();
                        }
                    });
                    ui.horizontal(|ui| {
                        if ui.button("Clone").clicked() {
                            self.clone_repo();
                        }
                        if ui.button("Cancel").clicked() {
                            self.show_clone = false;
                        }
                    });
                });
        }
    }
    fn repository_tabs(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let mut close = None;
            let mut activate = None;
            egui::ScrollArea::horizontal()
                .id_salt("repository_tabs")
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        for path in &self.open_repos {
                            let active = self.path.as_ref() == Some(path);
                            egui::Frame::new()
                                .fill(if active { PANEL2 } else { PANEL })
                                .inner_margin(6.)
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        if ui
                                            .selectable_label(
                                                active,
                                                path.file_name()
                                                    .unwrap_or_default()
                                                    .to_string_lossy(),
                                            )
                                            .on_hover_text(path.display().to_string())
                                            .clicked()
                                        {
                                            activate = Some(path.clone());
                                        }
                                        if ui
                                            .add_enabled(
                                                self.operation.is_none(),
                                                egui::Button::new("×").frame(false),
                                            )
                                            .on_hover_text("Close repository")
                                            .clicked()
                                        {
                                            close = Some(path.clone());
                                        }
                                    });
                                });
                        }
                        if ui
                            .add_enabled(
                                self.operation.is_none(),
                                egui::Button::new("+ Open").frame(false),
                            )
                            .clicked()
                            && let Some(path) = rfd::FileDialog::new().pick_folder()
                        {
                            activate = Some(path);
                        }
                    });
                });
            if let Some(path) = close {
                self.close_repo_tab(&path);
            } else if let Some(path) = activate {
                self.open(path);
            }
        });
    }
    fn sidebar(&mut self, ui: &mut egui::Ui) {
        ui.add_space(8.);
        ui.label(RichText::new("REPOSITORY").small().strong().color(MUTED));
        ui.add_space(6.);
        for (label, tab, count) in [
            ("History", 1, self.commits.len()),
            ("Uncommitted changes", 0, self.changes.len()),
            ("Files", 5, self.tracked_files.len()),
        ] {
            if ui
                .add_sized(
                    [ui.available_width(), 28.],
                    egui::Button::new(format!("{label}   {count}"))
                        .selected(self.tab == tab)
                        .frame(false),
                )
                .clicked()
            {
                self.tab = tab;
                if tab == 5
                    && let Some(path) = self
                        .selected_file
                        .clone()
                        .filter(|p| self.tracked_files.contains(p))
                        .or_else(|| self.tracked_files.first().cloned())
                {
                    self.preview_file(&path);
                }
                self.selected_commit = None;
                self.selected_commit_file = None;
                if tab == 0 {
                    self.selected_staged = false;
                    if let Some(c) = self
                        .changes
                        .iter()
                        .find(|c| !c.staged)
                        .cloned()
                        .or_else(|| self.changes.first().cloned())
                    {
                        self.selected_staged = c.staged;
                        self.select_file(c.path);
                    }
                }
            }
        }
        if ui
            .add_sized(
                [ui.available_width(), 26.],
                egui::Button::new("Repository tools")
                    .selected(self.tab == 6)
                    .frame(false),
            )
            .clicked()
        {
            self.tab = 6;
        }
        if ui
            .add_sized(
                [ui.available_width(), 26.],
                egui::Button::new("LFS / external tools")
                    .selected(self.tab == 8)
                    .frame(false),
            )
            .clicked()
        {
            self.tab = 8;
        }
        ui.add_space(14.);
        egui::CollapsingHeader::new(RichText::new("LOCAL BRANCHES").small().color(MUTED))
            .default_open(true)
            .show(ui, |ui| {
                for branch in self.branches.clone() {
                    let current = branch == self.branch;
                    let response = ui.add_sized(
                        [ui.available_width(), 25.],
                        egui::Button::new(
                            RichText::new(format!("{}  {branch}", if current { "•" } else { " " }))
                                .color(if current { GREEN } else { TEXT }),
                        )
                        .frame(false),
                    );
                    if response.clicked() {
                        self.set_history_scope(Some(format!("refs/heads/{branch}")));
                        self.tab = 1;
                    }
                    if response.double_clicked() && !current {
                        self.checkout(&branch);
                    }
                    response
                        .on_hover_text("Double-click to check out branch")
                        .context_menu(|ui| {
                            if ui
                                .add_enabled(
                                    !current && self.operation.is_none(),
                                    egui::Button::new("Check out"),
                                )
                                .clicked()
                            {
                                self.checkout(&branch);
                                ui.close_menu();
                            }
                            if ui.button("Manage branches…").clicked() {
                                self.tab = 2;
                                ui.close_menu();
                            }
                        });
                }
                if ui.small_button("+ Manage branches").clicked() {
                    self.tab = 2;
                }
            });
        ui.add_space(10.);
        egui::CollapsingHeader::new(RichText::new("REMOTES").small().color(MUTED))
            .default_open(true)
            .show(ui, |ui| {
                for branch in &self.remote_branches {
                    ui.label(RichText::new(branch).color(MUTED));
                }
                if ui.small_button("Manage remotes…").clicked() {
                    self.tab = 3;
                }
            });
        ui.add_space(10.);
        egui::CollapsingHeader::new(
            RichText::new(format!("TAGS   {}", self.tags.len()))
                .small()
                .color(MUTED),
        )
        .default_open(false)
        .show(ui, |ui| {
            for tag in &self.tags {
                ui.label(RichText::new(tag).color(BLUE));
            }
            if self.tags.is_empty() {
                ui.label(RichText::new("No tags").small().color(MUTED));
            }
        });
        ui.add_space(10.);
        egui::CollapsingHeader::new(
            RichText::new(format!("STASHES   {}", self.stashes.len()))
                .small()
                .color(MUTED),
        )
        .default_open(false)
        .show(ui, |ui| {
            for (index, message) in self.stashes.clone() {
                ui.label(RichText::new(&message).small())
                    .context_menu(|ui| {
                        let reference = format!("stash@{{{index}}}");
                        if ui.button("Apply").clicked() {
                            self.git_action(
                                "Apply stash",
                                &["stash", "apply", "--index", &reference],
                            );
                            ui.close_menu();
                        }
                        if ui.button("Pop").clicked() {
                            self.git_action("Pop stash", &["stash", "pop", "--index", &reference]);
                            ui.close_menu();
                        }
                        if ui.button("Drop…").clicked() {
                            self.pending_stash_drop = Some(index);
                            ui.close_menu();
                        }
                    });
            }
        });
    }
    fn content(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(match self.tab {
                    1 => "History",
                    2 => "Branches",
                    3 => "Remotes",
                    4 => "Blame",
                    5 => "Files",
                    6 => "Repository tools",
                    7 => "File history",
                    8 => "LFS and external tools",
                    _ => "Changes",
                })
                .size(15.)
                .strong()
                .color(TEXT),
            );
            if self.tab == 1 {
                self.history_display_options(ui);
                if ui.small_button("Back").clicked() {
                    self.navigate_history(false);
                }
                if ui.small_button("Next").clicked() {
                    self.navigate_history(true);
                }
                egui::ComboBox::from_id_salt("history_scope")
                    .width(165.)
                    .selected_text(
                        self.history_scope
                            .as_ref()
                            .map(|(name, _)| name.strip_prefix("refs/heads/").unwrap_or(name))
                            .unwrap_or("All branches"),
                    )
                    .show_ui(ui, |ui| {
                        if ui
                            .selectable_label(self.history_scope.is_none(), "All branches")
                            .clicked()
                        {
                            self.set_history_scope(None);
                        }
                        for branch in self.branches.clone() {
                            let name = format!("refs/heads/{branch}");
                            if ui
                                .selectable_label(
                                    self.history_scope.as_ref().is_some_and(|(n, _)| n == &name),
                                    &branch,
                                )
                                .clicked()
                            {
                                self.set_history_scope(Some(name));
                            }
                        }
                    });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.add_sized(
                        [190., 26.],
                        egui::TextEdit::singleline(&mut self.search)
                            .hint_text("Filter commits or authors…"),
                    );
                });
            } else if self.tab == 5 {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.add_sized(
                        [190., 26.],
                        egui::TextEdit::singleline(&mut self.file_search)
                            .hint_text("⌕  Filter files…"),
                    );
                });
            }
        });
        ui.separator();
        match self.tab {
            1 => self.history_view(ui),
            2 => self.branches_view(ui),
            3 => self.remotes_view(ui),
            4 => self.blame_view(ui),
            5 => self.files_view(ui),
            6 => {
                egui::ScrollArea::both().show(ui, |ui| self.repository_tools_view(ui));
            }
            7 => self.file_history_view(ui),
            8 => {
                egui::ScrollArea::both().show(ui, |ui| self.integrations_view(ui));
            }
            _ => self.changes_view(ui),
        }
        if let Some(path) = self.pending_discard.clone() {
            egui::Window::new("Discard changes?")
                .collapsible(false)
                .resizable(false)
                .show(ui.ctx(), |ui| {
                    ui.label(
                        RichText::new(format!(
                            "Discard uncommitted changes to {path}? This cannot be undone."
                        ))
                        .color(TEXT),
                    );
                    ui.horizontal(|ui| {
                        if ui.button("Cancel").clicked() {
                            self.pending_discard = None;
                        }
                        if ui
                            .add(
                                egui::Button::new(
                                    RichText::new("Discard").color(Color32::WHITE).strong(),
                                )
                                .fill(Color32::from_rgb(161, 62, 69)),
                            )
                            .clicked()
                        {
                            self.pending_discard = None;
                            self.discard(&path);
                        }
                    });
                });
        }
        if let Some(index) = self.pending_stash_drop {
            egui::Window::new("Drop stash?")
                .collapsible(false)
                .resizable(false)
                .show(ui.ctx(), |ui| {
                    ui.label(
                        RichText::new(format!(
                            "Permanently drop stash@{{{index}}}? This cannot be undone."
                        ))
                        .color(TEXT),
                    );
                    ui.horizontal(|ui| {
                        if ui.button("Cancel").clicked() {
                            self.pending_stash_drop = None;
                        }
                        if ui
                            .add(
                                egui::Button::new(
                                    RichText::new("Drop stash").color(Color32::WHITE).strong(),
                                )
                                .fill(Color32::from_rgb(161, 62, 69)),
                            )
                            .clicked()
                        {
                            self.pending_stash_drop = None;
                            let reference = format!("stash@{{{index}}}");
                            self.git_action("Drop stash", &["stash", "drop", &reference]);
                        }
                    });
                });
        }
        if let Some(path) = self.pending_init.clone() {
            egui::Window::new("Initialize Git repository?")
                .collapsible(false)
                .resizable(false)
                .show(ui.ctx(), |ui| {
                    ui.label(
                        RichText::new(format!(
                            "No Git repository was found in {}. Initialize one there? Existing files will stay in place.",
                            path.display()
                        ))
                        .color(TEXT),
                    );
                    ui.horizontal(|ui| {
                        if ui.button("Cancel").clicked() {
                            self.pending_init = None;
                        }
                        if ui.button("Initialize repository").clicked() {
                            self.pending_init = None;
                            self.init_repository(path);
                        }
                    });
                });
        }
        if let Some((upstream, mut plan)) = self.rebase_draft.clone() {
            let mut open = true;
            let mut run = false;
            let mut cancel = false;
            egui::Window::new(format!("Interactive rebase onto {upstream}"))
                .collapsible(false)
                .resizable(true)
                .default_width(720.)
                .open(&mut open)
                .show(ui.ctx(), |ui| {
                    ui.label(
                        RichText::new(
                            "Reorder lines or change pick to edit, squash, fixup, or drop. Keep a pick/edit commit before squash or fixup.",
                        )
                        .color(MUTED),
                    );
                    ui.add_space(8.);
                    ui.add_sized(
                        [ui.available_width(), 330.],
                        egui::TextEdit::multiline(&mut plan)
                            .font(egui::TextStyle::Monospace)
                            .desired_width(f32::INFINITY),
                    );
                    ui.horizontal(|ui| {
                        if ui.button("Cancel").clicked() {
                            cancel = true;
                        }
                        if ui
                            .add_enabled(!plan.trim().is_empty(), egui::Button::new("Run interactive rebase"))
                            .clicked()
                        {
                            run = true;
                        }
                    });
                });
            if cancel {
                open = false;
            }
            if run {
                self.start_interactive_rebase(&upstream, &plan);
            } else if open {
                self.rebase_draft = Some((upstream, plan));
            } else {
                self.rebase_draft = None;
            }
        }
    }
    fn changes_view(&mut self, ui: &mut egui::Ui) {
        if self.path.is_none() {
            self.empty(
                ui,
                "Open a repository",
                "Open or clone a repository to review changes.",
            );
            return;
        }
        if !self.focus_mode {
            egui::SidePanel::left("changes_files")
                .resizable(true)
                .default_width(310.)
                .width_range(240. ..=520.)
                .show_inside(ui, |ui| {
                    egui::TopBottomPanel::bottom("commit_editor")
                        .resizable(true)
                        .default_height(170.)
                        .height_range(150. ..=300.)
                        .show_inside(ui, |ui| self.commit_editor(ui));
                    let height = (ui.available_height() - 34.) * 0.5;
                    self.change_list(ui, false, height);
                    ui.separator();
                    self.change_list(ui, true, height);
                });
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::new().inner_margin(8.).fill(PANEL))
            .show_inside(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(self.selected_file.as_deref().unwrap_or("Diff preview"))
                            .strong(),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(
                            RichText::new(if self.selected_staged {
                                "Staged"
                            } else {
                                "Unstaged"
                            })
                            .small()
                            .color(MUTED),
                        );
                    });
                });
                ui.separator();
                egui::ScrollArea::vertical()
                    .id_salt("change_review")
                    .show(ui, |ui| {
                        self.conflict_controls(ui);
                        self.staging_controls(ui);
                        let height = ui.available_height().max(120.);
                        self.diff_panel(ui, height);
                    });
            });
    }
    fn history_view(&mut self, ui: &mut egui::Ui) {
        if self.path.is_none() {
            self.empty(
                ui,
                "History starts with a repository",
                "Open a repository to explore its commit graph.",
            );
            return;
        }
        let matching = match self.history_matches() {
            Ok(ids) => ids,
            Err(error) => {
                ui.colored_label(Color32::from_rgb(235, 139, 145), error);
                return;
            }
        };
        let first_parent = if self.history_options.first_parent {
            let start = self.repo().and_then(|r| {
                r.revparse_single(
                    self.history_scope
                        .as_ref()
                        .map(|(n, _)| n.as_str())
                        .unwrap_or("HEAD"),
                )
                .ok()
                .map(|o| o.id().to_string())
            });
            start.map(|id| history_features::first_parent_chain(&self.commits, &id))
        } else {
            None
        };
        let mut rows: Vec<_> = self
            .commits
            .iter()
            .filter(|c| {
                self.history_scope
                    .as_ref()
                    .is_none_or(|(_, ids)| ids.contains(&c.id))
                    && matching.contains(&c.id)
                    && first_parent.as_ref().is_none_or(|ids| ids.contains(&c.id))
                    && (!self.history_options.only_starred || self.is_starred(&c.id))
            })
            .cloned()
            .collect();
        if self.history_options.sort_by_date {
            rows.sort_by_key(|c| std::cmp::Reverse(c.timestamp));
        }
        if rows.is_empty() {
            let repository = self
                .path
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "No repository open".into());
            self.empty(
                ui,
                if self.history_error.is_some() {
                    "History unavailable"
                } else if self.commits.is_empty() {
                    "No commits yet"
                } else {
                    "No matching commits"
                },
                &format!(
                    "{}\nRepository: {repository}",
                    self.history_error
                        .as_deref()
                        .unwrap_or(if self.commits.is_empty() {
                            "This repository has no reachable commit history yet."
                        } else {
                            "Adjust the commit or author filter to see more history."
                        })
                ),
            );
            return;
        }
        let selection_visible = self
            .selected_commit
            .as_ref()
            .is_some_and(|id| rows.iter().any(|commit| &commit.id == id));
        if !selection_visible && let Some(commit) = rows.first() {
            self.select_history_commit(commit.id.clone());
        }
        let mut keyboard_target = None;
        if !ui.ctx().wants_keyboard_input() {
            let current = rows
                .iter()
                .position(|c| self.selected_commit.as_deref() == Some(&c.id))
                .unwrap_or(0);
            let target = ui.ctx().input_mut(|input| {
                if input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown) {
                    Some((current + 1).min(rows.len() - 1))
                } else if input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp) {
                    Some(current.saturating_sub(1))
                } else if input.consume_key(egui::Modifiers::NONE, egui::Key::Home) {
                    Some(0)
                } else if input.consume_key(egui::Modifiers::NONE, egui::Key::End) {
                    Some(rows.len() - 1)
                } else {
                    None
                }
            });
            if let Some(target) = target {
                self.select_history_commit(rows[target].id.clone());
                keyboard_target = Some(target);
            }
        }
        if self.focus_mode && !self.focused_history_table {
            self.commit_detail_view(ui);
            return;
        }
        // Keep all ancestors in the selected scope when computing graph lanes.
        // A text search must not change the topology or introduce unrelated branch lanes.
        let mut topology: Vec<_> = self
            .commits
            .iter()
            .filter(|c| {
                self.history_scope
                    .as_ref()
                    .is_none_or(|(_, ids)| ids.contains(&c.id))
            })
            .cloned()
            .collect();
        if let Some(ids) = &first_parent {
            topology.retain(|c| ids.contains(&c.id));
            for c in &mut topology {
                c.parents.truncate(1);
            }
        }
        if self.history_options.sort_by_date {
            topology.sort_by_key(|c| std::cmp::Reverse(c.timestamp));
        }
        let graph = graph_layout(&topology);
        let graph_by_id: HashMap<_, _> = topology
            .iter()
            .zip(&graph)
            .map(|(c, g)| (c.id.clone(), g))
            .collect();
        let lane_count = graph
            .iter()
            .map(|r| r.before.len().max(r.after.len()))
            .max()
            .unwrap_or(1);
        egui::TopBottomPanel::top(if self.focus_mode {
            "history_focused_table"
        } else {
            "history_table"
        })
        .resizable(true)
        .default_height(285.)
        .height_range(if self.focus_mode {
            ui.available_height()..=ui.available_height()
        } else {
            130. ..=(ui.available_height() - 230.).max(130.)
        })
        .show_inside(ui, |ui| {
            let graph_width = if self.history_options.hide_graph {
                24.
            } else {
                (lane_count as f32 * 16. + 16.).clamp(64., 180.)
            };
            let row_height = if self.history_options.compact {
                23.
            } else {
                28.
            };
            let width = ui
                .available_width()
                .min(ui.ctx().screen_rect().right() - ui.min_rect().left() - 8.);
            let hash_x = width
                - if self.history_options.hide_id {
                    0.
                } else {
                    66.
                };
            let date_x = hash_x
                - if self.history_options.hide_date {
                    0.
                } else {
                    99.
                };
            let author_x = date_x
                - if self.history_options.hide_author {
                    0.
                } else {
                    135.
                };
            let (header, _) = ui.allocate_exact_size(egui::vec2(width, 24.), egui::Sense::hover());
            for (x, label) in [
                (
                    8.,
                    if self.history_options.hide_graph {
                        ""
                    } else {
                        "Graph"
                    },
                ),
                (graph_width, "Description"),
                (
                    author_x,
                    if self.history_options.hide_author {
                        ""
                    } else {
                        "Author"
                    },
                ),
                (
                    date_x,
                    if self.history_options.hide_date {
                        ""
                    } else {
                        "Date"
                    },
                ),
                (
                    hash_x,
                    if self.history_options.hide_id {
                        ""
                    } else {
                        "Commit"
                    },
                ),
            ] {
                ui.painter().text(
                    egui::pos2(header.left() + x, header.center().y),
                    egui::Align2::LEFT_CENTER,
                    label,
                    FontId::proportional(11.),
                    MUTED,
                );
            }
            ui.separator();
            let mut picked = None;
            ui.spacing_mut().item_spacing.y = 0.;
            let scroll = egui::ScrollArea::vertical()
                .id_salt("commit_rows")
                .auto_shrink([false, false]);
            let scroll = if let Some(row) = keyboard_target {
                scroll.vertical_scroll_offset(row as f32 * row_height)
            } else {
                scroll
            };
            scroll.show_rows(ui, row_height, rows.len(), |ui, range| {
                for i in range {
                    let commit = &rows[i];
                    let (rect, response) =
                        ui.allocate_exact_size(egui::vec2(width, row_height), egui::Sense::click());
                    let selected = self.selected_commit.as_deref() == Some(&commit.id);
                    ui.painter().rect_filled(
                        rect,
                        0.,
                        if selected {
                            Color32::from_rgb(41, 65, 91)
                        } else if response.hovered() {
                            PANEL2
                        } else if i % 2 == 0 {
                            PANEL
                        } else {
                            BG
                        },
                    );
                    let graph_rect = egui::Rect::from_min_size(
                        rect.min + egui::vec2(8., 0.),
                        egui::vec2(graph_width - 12., row_height),
                    );
                    if !self.history_options.hide_graph
                        && let Some(row) = graph_by_id.get(commit.id.as_str())
                    {
                        draw_graph_row(&ui.painter().with_clip_rect(graph_rect), graph_rect, row);
                    }
                    let mut x = rect.left() + graph_width;
                    if self.is_starred(&commit.id) {
                        ui.painter().text(
                            egui::pos2(x, rect.center().y),
                            egui::Align2::LEFT_CENTER,
                            "★",
                            FontId::proportional(13.),
                            Color32::from_rgb(235, 190, 90),
                        );
                        x += 18.;
                    }
                    let message_clip = egui::Rect::from_min_max(
                        egui::pos2(x, rect.top()),
                        egui::pos2(rect.left() + author_x - 12., rect.bottom()),
                    );
                    let painter = ui.painter().with_clip_rect(message_clip);
                    for reference in &commit.refs {
                        let galley = painter.layout_no_wrap(
                            reference.clone(),
                            FontId::proportional(11.),
                            GREEN,
                        );
                        let badge = egui::Rect::from_min_size(
                            egui::pos2(x, rect.top() + 5.),
                            galley.size() + egui::vec2(10., 4.),
                        );
                        painter.rect_filled(badge, 3., Color32::from_rgb(37, 65, 56));
                        painter.galley(badge.min + egui::vec2(5., 2.), galley, GREEN);
                        x = badge.right() + 6.;
                    }
                    painter.text(
                        egui::pos2(x, rect.center().y),
                        egui::Align2::LEFT_CENTER,
                        &commit.message,
                        FontId::proportional(13.),
                        TEXT,
                    );
                    for (left, right, value, mono) in [
                        (author_x, date_x - 8., commit.author.as_str(), false),
                        (date_x, hash_x - 8., commit.date.as_str(), false),
                        (hash_x, width, commit.short.as_str(), true),
                    ] {
                        if right <= left {
                            continue;
                        }
                        let clip = egui::Rect::from_min_max(
                            egui::pos2(rect.left() + left, rect.top()),
                            egui::pos2(rect.left() + right, rect.bottom()),
                        );
                        ui.painter().with_clip_rect(clip).text(
                            egui::pos2(clip.left(), rect.center().y),
                            egui::Align2::LEFT_CENTER,
                            value,
                            if mono {
                                FontId::monospace(11.)
                            } else {
                                FontId::proportional(12.)
                            },
                            MUTED,
                        );
                    }
                    if response.clicked() {
                        self.focused_history_table = true;
                        picked = Some(commit.id.clone());
                    }
                    response.clone().context_menu(|ui| {
                        if ui
                            .button(if self.is_starred(&commit.id) {
                                "Remove star"
                            } else {
                                "Star commit"
                            })
                            .clicked()
                        {
                            self.toggle_star(&commit.id);
                            ui.close_menu();
                        }
                        if ui.button("Copy SHA").clicked() {
                            ui.ctx().copy_text(commit.id.clone());
                            ui.close_menu();
                        }
                    });
                    response.on_hover_text(format!(
                        "{}\n{} · {}\n{}",
                        commit.message, commit.author, commit.date, commit.id
                    ));
                }
            });
            if let Some(id) = picked {
                self.select_history_commit(id);
            }
        });
        if !self.focus_mode {
            egui::CentralPanel::default()
                .frame(egui::Frame::new().fill(PANEL).inner_margin(8.))
                .show_inside(ui, |ui| self.commit_detail_view(ui));
        }
    }
    fn commit_detail_view(&mut self, ui: &mut egui::Ui) {
        let Some(id) = self.selected_commit.clone() else {
            self.empty(
                ui,
                "Select a commit",
                "Commit details and changed files will appear here.",
            );
            return;
        };
        let Some(commit) = self.commits.iter().find(|commit| commit.id == id).cloned() else {
            self.empty(
                ui,
                "Commit unavailable",
                "Refresh the repository to reload its history.",
            );
            return;
        };
        ui.horizontal(|ui| {
            ui.label(RichText::new(&commit.message).strong());
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.small_button("Copy SHA").clicked() {
                    ui.ctx().copy_text(commit.id.clone());
                }
                ui.label(RichText::new(&commit.short).monospace().color(BLUE));
            });
        });
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!("{}  ·  {}", commit.author, commit.date))
                    .small()
                    .color(MUTED),
            );
            ui.menu_button("Commit actions", |ui| {
                ui.label(
                    RichText::new(format!("Commit {}", &id[..7.min(id.len())]))
                        .small()
                        .monospace()
                        .color(MUTED),
                );
                if ui
                    .add_enabled(self.operation.is_none(), egui::Button::new("Cherry-pick"))
                    .clicked()
                {
                    self.git_action("Cherry-pick", &["cherry-pick", &id]);
                }
                if ui
                    .add_enabled(self.operation.is_none(), egui::Button::new("Revert"))
                    .clicked()
                {
                    self.git_action("Revert", &["revert", "--no-edit", &id]);
                }
                if ui
                    .add_enabled(
                        self.operation.is_none(),
                        egui::Button::new("Checkout detached"),
                    )
                    .clicked()
                {
                    self.git_action("Checkout commit", &["checkout", "--detach", &id]);
                }
                ui.add_sized(
                    [130., 24.],
                    egui::TextEdit::singleline(&mut self.new_branch).hint_text("branch name"),
                );
                if ui
                    .add_enabled(
                        self.operation.is_none() && !self.new_branch.trim().is_empty(),
                        egui::Button::new("Create branch here"),
                    )
                    .clicked()
                {
                    let name = self.new_branch.trim().to_owned();
                    self.git_action("Create branch", &["checkout", "-b", &name, &id]);
                    self.new_branch.clear();
                }
            });
        });
        egui::CollapsingHeader::new("Message and metadata").show(ui, |ui| {
            ui.label(format!(
                "Author: {} <{}>",
                commit.author, commit.author_email
            ));
            ui.label(format!("Committer: {}", commit.committer));
            ui.label(format!("Parents: {}", commit.parents.join(", ")));
            egui::ScrollArea::vertical()
                .id_salt("full_commit_message")
                .max_height(100.)
                .show(ui, |ui| {
                    ui.label(&commit.full_message);
                });
            if ui
                .button(if self.is_starred(&id) {
                    "Remove star"
                } else {
                    "Star commit"
                })
                .clicked()
            {
                self.toggle_star(&id);
            }
        });
        ui.separator();
        let mut picked = None;
        let mut all = false;
        egui::SidePanel::left("commit_files")
            .resizable(true)
            .default_width(240.)
            .width_range(150. ..=420.)
            .show_inside(ui, |ui| {
                ui.label(
                    RichText::new(format!("Changed files   {}", self.commit_files.len()))
                        .small()
                        .strong()
                        .color(MUTED),
                );
                ui.separator();
                if ui
                    .selectable_label(self.selected_commit_file.is_none(), "All files")
                    .clicked()
                {
                    all = true;
                }
                if let Some(path) = self.selected_commit_file.clone()
                    && ui.small_button("File history…").clicked()
                {
                    self.open_file_history(path);
                }
                egui::ScrollArea::vertical()
                    .id_salt("commit_file_list")
                    .show(ui, |ui| {
                        for file in self.commit_files.clone() {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(file.status).monospace().color(
                                    match file.status {
                                        "A" => GREEN,
                                        "D" => Color32::from_rgb(235, 139, 145),
                                        _ => BLUE,
                                    },
                                ));
                                if ui
                                    .add_sized(
                                        [ui.available_width(), 24.],
                                        egui::Button::new(&file.path)
                                            .frame(false)
                                            .selected(
                                                self.selected_commit_file.as_deref()
                                                    == Some(&file.path),
                                            )
                                            .truncate(),
                                    )
                                    .on_hover_text(&file.path)
                                    .clicked()
                                {
                                    picked = Some(file.path.clone());
                                }
                            });
                        }
                    });
            });
        if all {
            self.selected_commit_file = None;
        }
        if let Some(path) = picked.as_ref() {
            self.selected_commit_file = Some(path.clone());
        }
        if all || picked.is_some() {
            self.diff = self
                .repo()
                .map(|r| diff_commit_path(&r, &id, self.selected_commit_file.as_deref()))
                .unwrap_or_default();
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(PANEL).inner_margin(8.))
            .show_inside(ui, |ui| {
                ui.label(
                    RichText::new(
                        self.selected_commit_file
                            .as_deref()
                            .unwrap_or("All changes"),
                    )
                    .small()
                    .color(MUTED),
                );
                ui.separator();
                self.diff_panel(ui, ui.available_height());
            });
    }
    fn branches_view(&mut self, ui: &mut egui::Ui) {
        if self.path.is_none() {
            self.empty(
                ui,
                "No repository open",
                "Open a repository to see its local branches.",
            );
            return;
        }
        ui.horizontal(|ui| {
            ui.label(RichText::new("Local branches").color(MUTED));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button("Create branch").clicked() {
                    let name = self.new_branch.trim().to_owned();
                    if !name.is_empty() {
                        self.git_action("Create branch", &["branch", &name]);
                        self.new_branch.clear();
                    }
                }
                ui.add_sized(
                    [180., 24.],
                    egui::TextEdit::singleline(&mut self.new_branch).hint_text("new branch name"),
                );
            });
        });
        ui.add_space(8.);
        ui.add_space(8.);
        for b in self.branches.clone() {
            egui::Frame::new()
                .fill(PANEL)
                .stroke(Stroke::new(1., BORDER))
                .corner_radius(8)
                .inner_margin(10.)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("⑂").color(BLUE));
                        ui.label(RichText::new(&b).strong().color(TEXT));
                        if b == self.branch {
                            ui.label(RichText::new("CURRENT").small().color(GREEN));
                        } else {
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if ui.small_button("Delete").clicked() {
                                    self.git_action("Delete branch", &["branch", "-d", &b]);
                                }
                                if ui.small_button("Merge into current").clicked() {
                                    self.git_action("Merge", &["merge", "--no-edit", &b]);
                                }
                                if ui
                                    .add_enabled(
                                        self.operation.is_none(),
                                        egui::Button::new("Rebase current onto"),
                                    )
                                    .clicked()
                                {
                                    self.git_action("Rebase", &["rebase", &b]);
                                }
                                if ui
                                    .add_enabled(
                                        self.operation.is_none() && self.changes.is_empty(),
                                        egui::Button::new("Interactive rebase…"),
                                    )
                                    .on_hover_text(
                                        "Edit commit order and choose commits to squash or drop; the working tree must be clean",
                                    )
                                    .clicked()
                                {
                                    self.prepare_interactive_rebase(&b);
                                }
                                if ui.button("Checkout").clicked() {
                                    self.checkout(&b)
                                }
                            });
                        }
                    })
                });
            ui.add_space(5.)
        }
        ui.separator();
        ui.label(
            RichText::new("REMOTE BRANCHES")
                .small()
                .strong()
                .color(MUTED),
        );
        if self.remote_branches.is_empty() {
            ui.label(
                RichText::new("Fetch a remote to list its branches.")
                    .small()
                    .color(MUTED),
            );
        }
        for branch in self.remote_branches.clone() {
            egui::Frame::new()
                .fill(PANEL)
                .stroke(Stroke::new(1., BORDER))
                .corner_radius(8)
                .inner_margin(8.)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("⇄").color(BLUE));
                        ui.label(RichText::new(&branch).color(TEXT).monospace());
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui
                                .add_enabled(
                                    self.operation.is_none(),
                                    egui::Button::new("Checkout as local branch"),
                                )
                                .clicked()
                            {
                                self.git_action(
                                    "Checkout remote branch",
                                    &["checkout", "--track", &branch],
                                );
                            }
                        });
                    });
                });
        }
        ui.separator();
        ui.horizontal(|ui| {
            ui.label(RichText::new("Create tag at HEAD").color(MUTED));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button("Create tag").clicked() {
                    let name = self.new_tag.trim().to_owned();
                    if !name.is_empty() {
                        self.git_action("Create tag", &["tag", &name]);
                        self.new_tag.clear();
                    }
                }
                ui.add_sized(
                    [180., 24.],
                    egui::TextEdit::singleline(&mut self.new_tag).hint_text("tag name"),
                );
            });
        });
        ui.add_space(8.);
        ui.label(RichText::new("Tags").color(MUTED));
        for tag in self.tags.clone() {
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("◆  {tag}")).color(GREEN));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.small_button("Manage tags…").clicked() {
                        self.tab = 6;
                    }
                });
            });
        }
    }
    fn remotes_view(&mut self, ui: &mut egui::Ui) {
        if self.path.is_none() {
            self.empty(
                ui,
                "No repository open",
                "Open a repository to configure remotes.",
            );
            return;
        }
        ui.horizontal(|ui| {
            ui.label(RichText::new("Repository remotes").color(MUTED));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui
                    .add_enabled(self.operation.is_none(), egui::Button::new("Add remote"))
                    .clicked()
                {
                    let name = self.remote_name.trim().to_owned();
                    let url = self.remote_url.trim().to_owned();
                    if !name.is_empty()
                        && !url.is_empty()
                        && self.operation.is_none()
                        && let Some(repo) = self.repo()
                    {
                        match repo.remote(&name, &url) {
                            Ok(_) => {
                                self.notice = format!("Remote {name} added");
                                self.remote_name.clear();
                                self.remote_url.clear();
                            }
                            Err(e) => self.notice = format!("Could not add remote: {e}"),
                        }
                    }
                }
                ui.add_sized(
                    [210., 24.],
                    egui::TextEdit::singleline(&mut self.remote_url)
                        .hint_text("https://… or SSH URL"),
                );
                ui.add_sized(
                    [100., 24.],
                    egui::TextEdit::singleline(&mut self.remote_name).hint_text("remote name"),
                );
            });
        });
        ui.add_space(8.);
        if let Some(repo) = self.repo()
            && let Ok(names) = repo.remotes()
        {
            for name in names.iter().flatten() {
                let url = repo
                    .find_remote(name)
                    .ok()
                    .and_then(|r| r.url().map(str::to_owned))
                    .unwrap_or_default();
                egui::Frame::new()
                    .fill(PANEL)
                    .stroke(Stroke::new(1., BORDER))
                    .corner_radius(8)
                    .inner_margin(10.)
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("⇄").color(BLUE));
                            ui.vertical(|ui| {
                                ui.label(RichText::new(name).strong().color(TEXT));
                                ui.label(RichText::new(&url).small().color(MUTED));
                            });
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if ui.small_button("Remove").clicked() {
                                    self.git_action("Remove remote", &["remote", "remove", name]);
                                }
                                if ui.button("Push").clicked() {
                                    self.git_action("Push", &["push", name]);
                                }
                                if ui.button("Pull").clicked() {
                                    self.git_action("Pull", &["pull", "--ff-only", name]);
                                }
                                if ui.button("Fetch").clicked() {
                                    self.git_action("Fetch", &["fetch", name, "--prune"]);
                                }
                            });
                        });
                    });
                ui.add_space(6.);
            }
        }
    }
    fn blame_view(&self, ui: &mut egui::Ui) {
        if self.aux_output.is_empty() {
            self.empty(
                ui,
                "Blame view",
                "Select a changed file and choose ‘Blame selected’.",
            );
            return;
        }
        egui::Frame::new()
            .fill(PANEL)
            .stroke(Stroke::new(1., BORDER))
            .corner_radius(8)
            .inner_margin(8.)
            .show(ui, |ui| {
                egui::ScrollArea::both().show(ui, |ui| {
                    for line in self.aux_output.lines() {
                        ui.label(RichText::new(line).font(FontId::monospace(12.)).color(TEXT));
                    }
                })
            });
    }
    fn files_view(&mut self, ui: &mut egui::Ui) {
        if self.path.is_none() {
            self.empty(
                ui,
                "No repository open",
                "Open a repository to browse its files.",
            );
            return;
        }
        let query = self.file_search.to_lowercase();
        let files: Vec<_> = self
            .tracked_files
            .iter()
            .filter(|p| p.to_lowercase().contains(&query))
            .cloned()
            .collect();
        egui::SidePanel::left("tracked_files")
            .resizable(true)
            .default_width(280.)
            .width_range(180. ..=450.)
            .show_inside(ui, |ui| {
                ui.label(
                    RichText::new(format!("Tracked files   {}", files.len()))
                        .small()
                        .color(MUTED),
                );
                ui.separator();
                egui::ScrollArea::vertical()
                    .id_salt("tracked_file_list")
                    .show(ui, |ui| {
                        for path in &files {
                            let response = ui.add_sized(
                                [ui.available_width(), 25.],
                                egui::Button::new(path)
                                    .selected(self.selected_file.as_deref() == Some(path))
                                    .frame(false)
                                    .truncate(),
                            );
                            if response.clicked() {
                                self.preview_file(path);
                            }
                            response.on_hover_text(path).context_menu(|ui| {
                                if ui.button("File history…").clicked() {
                                    self.open_file_history(path.clone());
                                    ui.close_menu();
                                }
                                if ui.button("Copy path").clicked() {
                                    ui.ctx().copy_text(path.clone());
                                    ui.close_menu();
                                }
                            });
                        }
                    });
            });
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(PANEL).inner_margin(8.))
            .show_inside(ui, |ui| {
                ui.label(
                    RichText::new(self.selected_file.as_deref().unwrap_or("Select a file"))
                        .strong(),
                );
                ui.horizontal_wrapped(|ui| {
                    if let Some(path) = self.selected_file.clone() {
                        if ui.button("File history…").clicked() {
                            self.open_file_history(path.clone());
                        }
                        if ui.button("Blame").clicked() {
                            self.git_action("Blame", &["blame", "--", &path]);
                            self.tab = 4;
                        }
                        if ui.button("External editor…").clicked() {
                            self.tab = 8;
                        }
                    }
                });
                ui.separator();
                egui::ScrollArea::both()
                    .id_salt("file_preview")
                    .show(ui, |ui| {
                        for (n, line) in self.file_content.lines().enumerate() {
                            ui.label(RichText::new(format!("{:>5}   {line}", n + 1)).monospace());
                        }
                    });
            });
    }
    fn empty(&self, ui: &mut egui::Ui, title: &str, sub: &str) {
        ui.add_space(60.);
        ui.vertical_centered(|ui| {
            ui.label(RichText::new("◈").size(32.).color(BLUE));
            ui.label(RichText::new(title).size(18.).strong().color(TEXT));
            ui.label(RichText::new(sub).color(MUTED));
        });
    }
}
fn shell_quote(value: &str) -> String {
    #[cfg(windows)]
    {
        format!("\"{}\"", value.replace('"', "\\\""))
    }
    #[cfg(not(windows))]
    {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}
fn statuses(r: &Repository) -> Vec<Change> {
    let mut o = StatusOptions::new();
    o.include_untracked(true)
        .recurse_untracked_dirs(true)
        .renames_head_to_index(true)
        .renames_index_to_workdir(true);
    r.statuses(Some(&mut o))
        .ok()
        .map(|ss| {
            ss.iter()
                .filter_map(|e| {
                    let s = e.status();
                    let path = e.path()?.to_owned();
                    let staged = s.intersects(
                        Status::INDEX_NEW
                            | Status::INDEX_MODIFIED
                            | Status::INDEX_DELETED
                            | Status::INDEX_RENAMED
                            | Status::INDEX_TYPECHANGE,
                    );
                    let index_status = if s.contains(Status::INDEX_NEW) {
                        "A"
                    } else if s.contains(Status::INDEX_DELETED) {
                        "D"
                    } else if s.contains(Status::INDEX_RENAMED) {
                        "R"
                    } else {
                        "M"
                    };
                    let worktree_status = if s.contains(Status::WT_NEW) {
                        "A"
                    } else if s.contains(Status::WT_DELETED) {
                        "D"
                    } else if s.contains(Status::WT_RENAMED) {
                        "R"
                    } else {
                        "M"
                    };
                    let worktree = s.intersects(
                        Status::WT_NEW
                            | Status::WT_MODIFIED
                            | Status::WT_DELETED
                            | Status::WT_RENAMED
                            | Status::WT_TYPECHANGE,
                    );
                    let mut rows = Vec::new();
                    if staged {
                        rows.push(Change {
                            path: path.clone(),
                            staged: true,
                            status: index_status,
                        });
                    }
                    if worktree {
                        rows.push(Change {
                            path: path.clone(),
                            staged: false,
                            status: worktree_status,
                        });
                    }
                    if rows.is_empty() && s.contains(Status::CONFLICTED) {
                        rows.push(Change {
                            path,
                            staged: false,
                            status: "!",
                        });
                    }
                    Some(rows)
                })
                .flatten()
                .collect()
        })
        .unwrap_or_default()
}
fn history(r: &Repository) -> Result<Vec<Commit>, String> {
    let mut w = r.revwalk().map_err(|error| error.message().to_owned())?;
    w.push_glob("refs/heads/*")
        .map_err(|error| error.message().to_owned())?;
    w.push_glob("refs/remotes/*")
        .map_err(|error| error.message().to_owned())?;
    w.push_glob("refs/tags/*")
        .map_err(|error| error.message().to_owned())?;
    if r.head().and_then(|head| head.peel_to_commit()).is_ok() {
        w.push_head().map_err(|error| error.message().to_owned())?;
    }
    w.set_sorting(git2::Sort::TIME | git2::Sort::TOPOLOGICAL)
        .map_err(|error| error.message().to_owned())?;
    let mut reference_names: HashMap<git2::Oid, Vec<String>> = HashMap::new();
    if let Ok(references) = r.references() {
        for reference in references.flatten() {
            if let (Ok(commit), Some(name)) = (reference.peel_to_commit(), reference.shorthand()) {
                reference_names
                    .entry(commit.id())
                    .or_default()
                    .push(name.to_owned());
            }
        }
    }
    let mut commits = Vec::new();
    for id in w {
        let id = id.map_err(|error| error.message().to_owned())?;
        let c = r
            .find_commit(id)
            .map_err(|error| error.message().to_owned())?;
        let date = chrono::DateTime::from_timestamp(c.time().seconds(), 0)
            .map(|d| d.format("%b %d, %Y").to_string())
            .unwrap_or_default();
        let refs = reference_names.get(&id).cloned().unwrap_or_default();
        let oid = id.to_string();
        let parents = c.parent_ids().map(|parent| parent.to_string()).collect();
        commits.push(Commit {
            id: oid.clone(),
            parents,
            short: oid[..7].into(),
            message: c.summary().unwrap_or("(no message)").into(),
            author: c.author().name().unwrap_or("Unknown").into(),
            author_email: c.author().email().unwrap_or("").into(),
            full_message: c.message().unwrap_or("").into(),
            committer: format!(
                "{} <{}>",
                c.committer().name().unwrap_or("Unknown"),
                c.committer().email().unwrap_or("")
            ),
            timestamp: c.time().seconds(),
            time_offset: c.time().offset_minutes(),
            date,
            refs,
        });
    }
    Ok(commits)
}
#[derive(Default)]
struct GraphRow {
    lane: usize,
    parents: Vec<usize>,
    before: Vec<Option<String>>,
    after: Vec<Option<String>>,
}
fn graph_layout(commits: &[Commit]) -> Vec<GraphRow> {
    let mut lanes: Vec<Option<String>> = Vec::new();
    let mut rows = Vec::with_capacity(commits.len());
    for commit in commits {
        let lane = if let Some(index) = lanes
            .iter()
            .position(|entry| entry.as_deref() == Some(&commit.id))
        {
            index
        } else if let Some(index) = lanes.iter().position(Option::is_none) {
            lanes[index] = Some(commit.id.clone());
            index
        } else {
            lanes.push(Some(commit.id.clone()));
            lanes.len() - 1
        };
        let before = lanes.clone();
        lanes[lane] = None;
        let mut parents = Vec::with_capacity(commit.parents.len());
        for (parent_index, parent) in commit.parents.iter().enumerate() {
            let parent_lane = if let Some(index) = lanes
                .iter()
                .position(|entry| entry.as_deref() == Some(parent))
            {
                index
            } else if parent_index == 0 {
                lane
            } else if let Some(index) = lanes.iter().position(Option::is_none) {
                index
            } else {
                lanes.push(None);
                lanes.len() - 1
            };
            lanes[parent_lane] = Some(parent.clone());
            parents.push(parent_lane);
        }
        rows.push(GraphRow {
            lane,
            parents,
            before,
            after: lanes.clone(),
        });
    }
    rows
}
fn draw_graph_row(painter: &egui::Painter, rect: egui::Rect, row: &GraphRow) {
    const COLORS: [Color32; 6] = [
        BLUE,
        GREEN,
        Color32::from_rgb(229, 158, 91),
        Color32::from_rgb(196, 132, 230),
        Color32::from_rgb(92, 197, 205),
        Color32::from_rgb(237, 119, 141),
    ];
    let x_for = |lane: usize| rect.left() + 8. + lane as f32 * 16.;
    let center_y = rect.center().y;
    let width = row.before.len().max(row.after.len());
    for lane in 0..width {
        let x = x_for(lane);
        let color = COLORS[lane % COLORS.len()];
        if lane == row.lane {
            painter.line_segment(
                [egui::pos2(x, rect.top()), egui::pos2(x, center_y)],
                Stroke::new(2., color),
            );
            if row.parents.contains(&lane) {
                painter.line_segment(
                    [egui::pos2(x, center_y), egui::pos2(x, rect.bottom())],
                    Stroke::new(2., color),
                );
            }
        } else if row.parents.contains(&lane) {
            painter.line_segment(
                [
                    egui::pos2(x_for(row.lane), center_y),
                    egui::pos2(x, center_y),
                ],
                Stroke::new(2., color),
            );
            painter.line_segment(
                [egui::pos2(x, center_y), egui::pos2(x, rect.bottom())],
                Stroke::new(2., color),
            );
        } else if row.before.get(lane).is_some_and(Option::is_some)
            || row.after.get(lane).is_some_and(Option::is_some)
        {
            painter.line_segment(
                [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
                Stroke::new(2., color),
            );
        }
    }
    painter.circle_filled(
        egui::pos2(x_for(row.lane), center_y),
        5.,
        COLORS[row.lane % COLORS.len()],
    );
}
fn diff_file_side(r: &Repository, path: &str, staged: bool) -> String {
    let mut out = String::new();
    let mut options = git2::DiffOptions::new();
    options
        .pathspec(path)
        .disable_pathspec_match(true)
        .include_untracked(true)
        .recurse_untracked_dirs(true)
        .show_untracked_content(true);
    let diff = if staged {
        let tree = r.head().ok().and_then(|h| h.peel_to_tree().ok());
        r.diff_tree_to_index(tree.as_ref(), None, Some(&mut options))
    } else {
        r.diff_index_to_workdir(None, Some(&mut options))
    };
    match diff {
        Ok(diff) => append_diff(&mut out, &diff),
        Err(error) => out = format!("Could not read diff: {error}"),
    }
    out
}
fn unstaged_patch(repo: &Repository, path: &str) -> Result<String, String> {
    let mut options = git2::DiffOptions::new();
    options
        .pathspec(path)
        .disable_pathspec_match(true)
        .context_lines(3);
    let diff = repo
        .diff_index_to_workdir(None, Some(&mut options))
        .map_err(|error| error.message().to_owned())?;
    let mut patch = String::new();
    diff.print(git2::DiffFormat::Patch, |_, _, line| {
        if matches!(line.origin(), '+' | '-' | ' ') {
            patch.push(line.origin());
        }
        patch.push_str(std::str::from_utf8(line.content()).unwrap_or(""));
        true
    })
    .map_err(|error| error.message().to_owned())?;
    if patch.len() > 4 * 1024 * 1024 {
        return Err("Patch is larger than the 4 MB editor limit".into());
    }
    Ok(patch)
}
fn patch_has_changes(patch: &str) -> bool {
    !diff_features::selectable_lines(patch).is_empty()
}
fn unstaged_hunks(repo: &Repository, path: &str) -> Vec<(u32, u32, u32, u32)> {
    let mut options = git2::DiffOptions::new();
    options
        .pathspec(path)
        .disable_pathspec_match(true)
        .include_untracked(true)
        .recurse_untracked_dirs(true);
    let Ok(diff) = repo.diff_index_to_workdir(None, Some(&mut options)) else {
        return Vec::new();
    };
    let mut hunks = Vec::new();
    let mut file_cb = |_: git2::DiffDelta<'_>, _: f32| true;
    let mut hunk_cb = |_: git2::DiffDelta<'_>, hunk: git2::DiffHunk<'_>| {
        hunks.push((
            hunk.old_start(),
            hunk.new_start(),
            hunk.old_lines(),
            hunk.new_lines(),
        ));
        true
    };
    let _ = diff.foreach(&mut file_cb, None, Some(&mut hunk_cb), None);
    hunks
}
fn append_diff(out: &mut String, diff: &git2::Diff<'_>) {
    let _ = diff.print(git2::DiffFormat::Patch, |_, _, l| {
        if matches!(l.origin(), '+' | '-' | ' ') {
            out.push(l.origin());
        }
        out.push_str(std::str::from_utf8(l.content()).unwrap_or(""));
        true
    });
}
fn commit_files(r: &Repository, id: &str) -> Vec<CommitFile> {
    let Some(c) = r
        .revparse_single(id)
        .ok()
        .and_then(|o| o.peel_to_commit().ok())
    else {
        return Vec::new();
    };
    let old = c.parent(0).ok().and_then(|p| p.tree().ok());
    let new = c.tree().ok();
    let mut files = Vec::new();
    if let Ok(diff) = r.diff_tree_to_tree(old.as_ref(), new.as_ref(), None) {
        for delta in diff.deltas() {
            let path = delta
                .new_file()
                .path()
                .or_else(|| delta.old_file().path())
                .map(|path| path.to_string_lossy().into_owned());
            let Some(path) = path else { continue };
            let status = match delta.status() {
                git2::Delta::Added => "A",
                git2::Delta::Deleted => "D",
                git2::Delta::Renamed => "R",
                git2::Delta::Copied => "C",
                git2::Delta::Typechange => "T",
                _ => "M",
            };
            files.push(CommitFile { path, status });
        }
    }
    files
}
fn diff_commit_path(r: &Repository, id: &str, path: Option<&str>) -> String {
    let Some(c) = r
        .revparse_single(id)
        .ok()
        .and_then(|o| o.peel_to_commit().ok())
    else {
        return String::new();
    };
    let old = c.parent(0).ok().and_then(|p| p.tree().ok());
    let new = c.tree().ok();
    let mut s = String::new();
    let mut options = git2::DiffOptions::new();
    if let Some(path) = path {
        options.pathspec(path).disable_pathspec_match(true);
    }
    if let Ok(d) = r.diff_tree_to_tree(old.as_ref(), new.as_ref(), Some(&mut options)) {
        let _ = d.print(git2::DiffFormat::Patch, |_, _, l| {
            if matches!(l.origin(), '+' | '-' | ' ') {
                s.push(l.origin());
            }
            s.push_str(std::str::from_utf8(l.content()).unwrap_or(""));
            true
        });
    }
    s
}
impl eframe::App for GritApp {
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        let paths = self
            .recent_repos
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join("\n");
        storage.set_string("recent_repositories", paths);
        storage.set_string("external_editor", self.integrations.editor_program.clone());
        storage.set_string("commit_template", self.commit_options.template.clone());
        storage.set_string(
            "commit_template_name",
            self.commit_options.template_name.clone(),
        );
        let templates: Vec<String> = self
            .commit_options
            .templates
            .iter()
            .flat_map(|(n, v)| [n.clone(), v.clone()])
            .collect();
        storage.set_string(
            "commit_templates",
            history_features::encode_records(&templates),
        );
        storage.set_string(
            "commit_author_name",
            self.commit_options.author_name.clone(),
        );
        storage.set_string(
            "commit_author_email",
            self.commit_options.author_email.clone(),
        );
        storage.set_string("commit_signoff", self.commit_options.signoff.to_string());
        for (key, value) in [
            ("history_first_parent", self.history_options.first_parent),
            ("history_only_starred", self.history_options.only_starred),
            ("history_sort_date", self.history_options.sort_by_date),
            ("history_compact", self.history_options.compact),
            ("history_hide_graph", self.history_options.hide_graph),
            ("history_hide_author", self.history_options.hide_author),
            ("history_hide_date", self.history_options.hide_date),
            ("history_hide_id", self.history_options.hide_id),
            ("hide_untracked", self.history_options.hide_untracked),
            ("diff_wrap", self.wrap_diff),
        ] {
            storage.set_string(key, value.to_string());
        }
        for (repo, stars) in &self.history_options.stars {
            storage.set_string(
                &format!("commit_stars:{repo}"),
                stars.iter().cloned().collect::<Vec<_>>().join("\n"),
            );
        }

        let open_paths = self
            .open_repos
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join("\n");
        storage.set_string("open_repositories", open_paths);
        storage.set_string(
            "active_repository",
            self.path
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_default(),
        );
    }
    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        let finished = self
            .operation
            .as_ref()
            .and_then(|(label, rx)| match rx.try_recv() {
                Ok(result) => Some((label.clone(), result)),
                Err(mpsc::TryRecvError::Disconnected) => {
                    Some((label.clone(), Err("Operation stopped unexpectedly".into())))
                }
                Err(mpsc::TryRecvError::Empty) => None,
            });
        if let Some((label, result)) = finished {
            self.operation = None;
            if label.contains("LFS") {
                self.integrations.invalidate();
            }
            self.operation_log.push(format!(
                "{label}: {}",
                match &result {
                    Ok(output) =>
                        if output.is_empty() {
                            "Completed"
                        } else {
                            output.as_str()
                        },
                    Err(error) => error.as_str(),
                }
            ));
            if self.operation_log.len() > 100 {
                self.operation_log.remove(0);
            }
            match result {
                Ok(output) if label == "Blame" => {
                    self.aux_output = output;
                    self.notice = "Blame loaded".into();
                }
                Ok(output) if label == "Commit" => {
                    self.message.clear();
                    self.amend = false;
                    self.selected_commit = None;
                    self.notice = if output.is_empty() {
                        "Commit created".into()
                    } else {
                        output
                    };
                    self.refresh();
                }
                Ok(_) if label == "Stage edited patch" => {
                    self.patch_draft = None;
                    self.notice = "Selected patch staged".into();
                    self.refresh();
                }
                Ok(output) => {
                    self.notice = if output.is_empty() {
                        format!("{label} completed")
                    } else {
                        format!("{label}: {output}")
                    };
                    if let Some(path) = self.open_after_operation.take() {
                        self.open(path)
                    } else {
                        self.refresh()
                    }
                }
                Err(error) => {
                    self.open_after_operation = None;
                    self.notice = format!("{label} failed: {error}");
                    self.refresh();
                }
            }
        } else if self.operation.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        let focused = ctx.input(|i| i.focused);
        if focused && !self.was_focused && self.operation.is_none() {
            self.refresh();
        }
        self.was_focused = focused;
        if self.operation.is_none()
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::Q))
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        let dropped = ctx.input(|input| {
            input
                .raw
                .dropped_files
                .iter()
                .find_map(|file| file.path.clone())
        });
        if self.operation.is_none()
            && let Some(path) = dropped
        {
            self.open(path);
        }
        if ctx.input_mut(|input| input.consume_key(egui::Modifiers::COMMAND, egui::Key::O))
            && let Some(path) = rfd::FileDialog::new().pick_folder()
        {
            self.open(path);
        }
        if ctx.input_mut(|input| input.consume_key(egui::Modifiers::COMMAND, egui::Key::R))
            && self.operation.is_none()
        {
            self.refresh();
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::M)) {
            self.focus_mode = !self.focus_mode;
        }
        if self.show_commit_options {
            let mut open = true;
            egui::Window::new("Commit templates and author")
                .open(&mut open)
                .default_width(520.)
                .show(ctx, |ui| {
                    ui.set_max_width(600.);
                    egui::ScrollArea::vertical()
                        .max_height(500.)
                        .show(ui, |ui| self.commit_options_ui(ui));
                });
            self.show_commit_options = open;
        }
        if self.show_operation_log {
            egui::Window::new("Operation log")
                .open(&mut self.show_operation_log)
                .default_width(700.)
                .show(ctx, |ui| {
                    egui::ScrollArea::both().max_height(450.).show(ui, |ui| {
                        for entry in self.operation_log.iter().rev() {
                            ui.label(RichText::new(entry).monospace());
                            ui.separator();
                        }
                    });
                });
        }
        egui::TopBottomPanel::top("toolbar")
            .frame(
                egui::Frame::new()
                    .fill(PANEL)
                    .inner_margin(egui::Margin::symmetric(12, 8)),
            )
            .show(ctx, |ui| {
                self.header(ui);
            });
        egui::TopBottomPanel::top("repositories")
            .frame(
                egui::Frame::new()
                    .fill(BG)
                    .inner_margin(egui::Margin::symmetric(8, 0)),
            )
            .show(ctx, |ui| self.repository_tabs(ui));
        egui::TopBottomPanel::bottom("status")
            .frame(
                egui::Frame::new()
                    .fill(PANEL)
                    .inner_margin(egui::Margin::symmetric(10, 5)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.add(
                        egui::Label::new(RichText::new(&self.notice).small().color(MUTED))
                            .truncate(),
                    )
                    .on_hover_text(&self.notice);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if let Some(path) = &self.path {
                            ui.add(
                                egui::Label::new(
                                    RichText::new(path.display().to_string())
                                        .small()
                                        .color(MUTED),
                                )
                                .truncate(),
                            );
                        }
                    });
                });
            });
        if !self.focus_mode {
            egui::SidePanel::left("references")
                .resizable(true)
                .default_width(220.)
                .width_range(180. ..=340.)
                .frame(egui::Frame::new().fill(PANEL).inner_margin(10.))
                .show(ctx, |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| self.sidebar(ui));
                });
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(BG).inner_margin(8.))
            .show(ctx, |ui| {
                ui.add_enabled_ui(self.operation.is_none(), |ui| self.content(ui));
            });
    }
}
mod commit_features;
mod diff_features;
mod file_features;
mod history_features;
mod integration_features;
mod repository_features;
#[cfg(test)]
mod tests;
mod ui;

fn main() -> eframe::Result<()> {
    let mut args = std::env::args_os().skip(1);
    if args.next().as_deref() == Some(std::ffi::OsStr::new("--sequence-editor")) {
        let Some(todo_path) = args.next() else {
            eprintln!("Git did not provide a rebase todo file");
            std::process::exit(1);
        };
        let Some(draft_path) = std::env::var_os("GRIT_SEQUENCE_DRAFT") else {
            eprintln!("The grit rebase plan is unavailable");
            std::process::exit(1);
        };
        if let Err(error) = std::fs::copy(draft_path, todo_path) {
            eprintln!("Could not apply the grit rebase plan: {error}");
            std::process::exit(1);
        }
        return Ok(());
    }
    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1320., 860.])
            .with_min_inner_size([980., 680.])
            .with_title("grit — Git client"),
        ..Default::default()
    };
    eframe::run_native(
        "grit",
        opts,
        Box::new(|context| {
            ui::configure_style(&context.egui_ctx);
            let mut app = GritApp {
                tab: 1,
                ..Default::default()
            };
            let mut active_path = None;
            if let Some(storage) = context.storage {
                if let Some(recent) = storage.get_string("recent_repositories") {
                    app.recent_repos = recent
                        .lines()
                        .map(PathBuf::from)
                        .filter(|path| path.exists())
                        .take(8)
                        .collect();
                }
                if let Some(open) = storage.get_string("open_repositories") {
                    app.open_repos = open
                        .lines()
                        .map(PathBuf::from)
                        .filter(|path| Repository::open(path).is_ok())
                        .collect();
                }
                app.commit_options.template =
                    storage.get_string("commit_template").unwrap_or_default();
                app.commit_options.template_name = storage
                    .get_string("commit_template_name")
                    .unwrap_or_default();
                if let Some(saved) = storage.get_string("commit_templates")
                    && let Ok(records) = history_features::decode_records(&saved)
                {
                    app.commit_options.templates = records
                        .chunks_exact(2)
                        .map(|r| (r[0].clone(), r[1].clone()))
                        .collect();
                }
                app.commit_options.author_name =
                    storage.get_string("commit_author_name").unwrap_or_default();
                app.commit_options.author_email = storage
                    .get_string("commit_author_email")
                    .unwrap_or_default();
                app.commit_options.signoff =
                    storage.get_string("commit_signoff").as_deref() == Some("true");
                for (key, target) in [
                    (
                        "history_first_parent",
                        &mut app.history_options.first_parent,
                    ),
                    (
                        "history_only_starred",
                        &mut app.history_options.only_starred,
                    ),
                    ("history_sort_date", &mut app.history_options.sort_by_date),
                    ("history_compact", &mut app.history_options.compact),
                    ("history_hide_graph", &mut app.history_options.hide_graph),
                    ("history_hide_author", &mut app.history_options.hide_author),
                    ("history_hide_date", &mut app.history_options.hide_date),
                    ("history_hide_id", &mut app.history_options.hide_id),
                    ("hide_untracked", &mut app.history_options.hide_untracked),
                    ("diff_wrap", &mut app.wrap_diff),
                ] {
                    *target = storage.get_string(key).as_deref() == Some("true");
                }
                for path in app.open_repos.iter().chain(&app.recent_repos) {
                    let key = history_features::repo_storage_key(path);
                    let stars = storage
                        .get_string(&format!("commit_stars:{key}"))
                        .unwrap_or_default()
                        .lines()
                        .filter(|s| s.len() >= 40 && s.chars().all(|c| c.is_ascii_hexdigit()))
                        .map(str::to_owned)
                        .collect();
                    app.history_options.stars.insert(key, stars);
                }
                active_path = storage
                    .get_string("active_repository")
                    .map(PathBuf::from)
                    .filter(|path| app.open_repos.contains(path));
            }
            if app.commit_options.template.is_empty()
                && let Some((name, value)) = app.commit_options.templates.first()
            {
                app.commit_options.template_name = name.clone();
                app.commit_options.template = value.clone();
            }
            if let Some(path) = std::env::args_os()
                .nth(1)
                .map(PathBuf::from)
                .or(active_path)
                .or_else(|| app.open_repos.last().cloned())
                .or_else(|| app.recent_repos.first().cloned())
            {
                app.open(path);
            }
            Ok(Box::new(app))
        }),
    )
}
