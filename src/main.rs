#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use eframe::egui::{self, Align, Color32, FontId, Layout, RichText, Stroke};
use git2::{BranchType, Repository, Status, StatusOptions};
use std::{
    collections::HashMap,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc::{self, Receiver},
};

const BG: Color32 = Color32::from_rgb(20, 23, 29);
const PANEL: Color32 = Color32::from_rgb(27, 31, 39);
const PANEL2: Color32 = Color32::from_rgb(33, 38, 47);
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
#[derive(Clone)]
struct Commit {
    id: String,
    parents: Vec<String>,
    short: String,
    message: String,
    author: String,
    date: String,
    refs: Vec<String>,
}
#[derive(Default)]
struct GritApp {
    path: Option<PathBuf>,
    open_repos: Vec<PathBuf>,
    recent_repos: Vec<PathBuf>,
    commits: Vec<Commit>,
    history_error: Option<String>,
    changes: Vec<Change>,
    stashes: Vec<(usize, String)>,
    tracked_files: Vec<String>,
    branches: Vec<String>,
    remote_branches: Vec<String>,
    tags: Vec<String>,
    branch: String,
    selected_commit: Option<String>,
    selected_file: Option<String>,
    file_content: String,
    diff: String,
    message: String,
    search: String,
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
                self.refresh();
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
        self.selected_file = None;
        self.file_content.clear();
        self.diff.clear();
        self.aux_output.clear();
        self.conflict_draft = None;
        self.patch_draft = None;
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
        self.branch = repo
            .head()
            .ok()
            .and_then(|h| h.shorthand().map(str::to_owned))
            .unwrap_or_else(|| "(detached HEAD)".into());
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
            self.diff = diff_commit(&repo, &id)
        } else if let Some(path) = self.changes.first().map(|c| c.path.clone()) {
            self.select_file(path)
        } else {
            self.selected_file = None;
            self.diff.clear()
        }
    }
    fn select_file(&mut self, p: String) {
        self.selected_file = Some(p.clone());
        if let Some(r) = self.repo() {
            self.diff = diff_file(&r, &p)
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
                i.add_path(Path::new(p))
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
            self.git_action(
                "Discard changes",
                &[
                    "restore",
                    "--source=HEAD",
                    "--staged",
                    "--worktree",
                    "--",
                    path,
                ],
            );
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
    fn apply_stage_patch(&mut self, patch: &str) {
        if self.operation.is_some() {
            return;
        }
        if patch.trim().is_empty() {
            self.notice = "The patch is empty".into();
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
        self.notice = "Stage edited patch in progress…".into();
        self.operation = Some(("Stage edited patch".into(), rx));
    }
    fn commit(&mut self) {
        let msg = self.message.trim().to_owned();
        if msg.is_empty() {
            self.notice = "Enter a commit message first".into();
            return;
        }
        let mut args = vec!["commit".to_owned()];
        if self.amend {
            args.push("--amend".into());
        }
        args.extend(["-m".into(), msg]);
        self.start_git_command("Commit", args);
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
            ui.label(RichText::new("grit").size(21.).strong().color(TEXT));
            ui.label(RichText::new("●").color(BLUE));
            ui.separator();
            if ui
                .add_enabled(
                    self.operation.is_none(),
                    egui::Button::new("＋  Open repository"),
                )
                .clicked()
                && let Some(p) = rfd::FileDialog::new().pick_folder()
            {
                self.open(p)
            }
            if ui
                .add_enabled(
                    self.operation.is_none(),
                    egui::Button::new("Init repository"),
                )
                .clicked()
                && let Some(p) = rfd::FileDialog::new().pick_folder()
            {
                self.init_repository(p)
            }
            if ui
                .add_enabled(self.path.is_some(), egui::Button::new("↻  Refresh"))
                .clicked()
            {
                self.refresh()
            }
            if ui
                .add_enabled(self.operation.is_none(), egui::Button::new("↓ Clone"))
                .clicked()
            {
                self.show_clone = true;
            }
            let connected = self.path.is_some();
            if ui
                .add_enabled(connected, egui::Button::new("Fetch"))
                .clicked()
            {
                self.git_action("Fetch", &["fetch", "--all", "--prune"]);
            }
            if ui
                .add_enabled(connected, egui::Button::new("Pull"))
                .clicked()
            {
                self.git_action("Pull", &["pull", "--ff-only"]);
            }
            if ui
                .add_enabled(connected, egui::Button::new("Push"))
                .clicked()
            {
                self.git_action("Push", &["push"]);
            }
            if ui
                .add_enabled(
                    connected && self.operation.is_none(),
                    egui::Button::new("Blame selected"),
                )
                .clicked()
                && let Some(path) = self.selected_file.clone()
            {
                self.git_action("Blame", &["blame", "--", &path]);
                self.tab = 4;
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(
                    RichText::new(if self.branch.is_empty() {
                        "No repository"
                    } else {
                        &self.branch
                    })
                    .color(BLUE)
                    .monospace(),
                );
                ui.label(RichText::new("branch").color(MUTED).small());
            });
        });
        if !self.open_repos.is_empty() {
            ui.horizontal_wrapped(|ui| {
                let mut switch_to = None;
                let mut close = None;
                for path in self.open_repos.clone() {
                    let active = self.path.as_ref() == Some(&path);
                    let label = path.file_name().unwrap_or_default().to_string_lossy();
                    ui.horizontal(|ui| {
                        if ui
                            .selectable_label(
                                active,
                                RichText::new(label).color(if active { TEXT } else { MUTED }),
                            )
                            .on_hover_text(path.display().to_string())
                            .clicked()
                        {
                            switch_to = Some(path.clone());
                        }
                        if ui
                            .add_enabled(self.operation.is_none(), egui::Button::new("×").small())
                            .clicked()
                        {
                            close = Some(path.clone());
                        }
                    });
                }
                if let Some(path) = close {
                    self.close_repo_tab(&path);
                } else if let Some(path) = switch_to
                    && self.path.as_ref() != Some(&path)
                {
                    self.open(path);
                }
            });
        }
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
    fn sidebar(&mut self, ui: &mut egui::Ui) {
        ui.set_min_width(205.);
        if let Some(p) = &self.path {
            ui.label(
                RichText::new(p.file_name().unwrap_or_default().to_string_lossy())
                    .strong()
                    .color(TEXT),
            );
            ui.label(RichText::new(p.display().to_string()).small().color(MUTED));
        } else {
            ui.label(
                RichText::new("YOUR WORKSPACE")
                    .small()
                    .strong()
                    .color(MUTED),
            );
            ui.add_space(12.);
            ui.label(
                RichText::new("RECENT REPOSITORIES")
                    .small()
                    .strong()
                    .color(MUTED),
            );
            for path in self.recent_repos.clone() {
                let title = path.file_name().unwrap_or_default().to_string_lossy();
                if ui
                    .selectable_label(false, RichText::new(format!("◈  {title}")).color(TEXT))
                    .on_hover_text(path.display().to_string())
                    .clicked()
                {
                    self.open(path);
                }
            }
        }
        ui.add_space(14.);
        ui.label(RichText::new("WORKING TREE").small().strong().color(MUTED));
        let count = self.changes.len();
        for (icon, label, n, tab) in [
            ("◈", "History", self.commits.len(), 1),
            ("◉", "Changes", count, 0),
            ("⑂", "Branches", self.branches.len(), 2),
            ("⇄", "Remotes", 0, 3),
            ("▤", "Blame", 0, 4),
            ("▣", "Files", self.tracked_files.len(), 5),
        ] {
            ui.horizontal(|ui| {
                if ui
                    .selectable_label(
                        self.tab == tab,
                        RichText::new(format!("{icon}  {label}")).color(if self.tab == tab {
                            TEXT
                        } else {
                            MUTED
                        }),
                    )
                    .clicked()
                {
                    self.tab = tab;
                    self.selected_commit = None
                }
                ui.label(RichText::new(n.to_string()).small().color(MUTED));
            });
        }
        ui.add_space(12.);
        ui.label(
            RichText::new("LOCAL BRANCHES")
                .small()
                .strong()
                .color(MUTED),
        );
        egui::ScrollArea::vertical()
            .max_height(280.)
            .show(ui, |ui| {
                for b in self.branches.clone() {
                    let current = b == self.branch;
                    if ui
                        .selectable_label(
                            current,
                            RichText::new(format!("  ●  {b}")).color(if current {
                                GREEN
                            } else {
                                TEXT
                            }),
                        )
                        .clicked()
                        && !current
                    {
                        self.checkout(&b)
                    }
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
                    _ => "Changes",
                })
                .size(18.)
                .strong()
                .color(TEXT),
            );
            if self.tab == 1 {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.add_sized(
                        [190., 26.],
                        egui::TextEdit::singleline(&mut self.search)
                            .hint_text("⌕  Filter commits…"),
                    );
                });
            }
        });
        ui.add_space(10.);
        match self.tab {
            1 => self.history_view(ui),
            2 => self.branches_view(ui),
            3 => self.remotes_view(ui),
            4 => self.blame_view(ui),
            5 => self.files_view(ui),
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
    }
    fn changes_view(&mut self, ui: &mut egui::Ui) {
        if self.path.is_none() {
            self.empty(
                ui,
                "Open a local repository",
                "Branches, commits and changes will appear here.",
            );
            return;
        }
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("Changes  {}", self.changes.len())).color(MUTED));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button("Stash pop").clicked() {
                    self.git_action("Stash pop", &["stash", "pop"]);
                }
                if ui.button("Stash save").clicked() {
                    self.git_action("Stash", &["stash", "push", "-u", "-m", "Saved from grit"]);
                }
                let merging = self
                    .repo()
                    .is_some_and(|r| r.state() == git2::RepositoryState::Merge);
                if ui
                    .add_enabled(merging, egui::Button::new("Abort merge"))
                    .clicked()
                {
                    self.git_action("Abort merge", &["merge", "--abort"]);
                }
                if ui
                    .add_enabled(self.operation.is_none(), egui::Button::new("Stage all  →"))
                    .clicked()
                {
                    let paths: Vec<_> = self
                        .changes
                        .iter()
                        .filter(|c| !c.staged)
                        .map(|c| c.path.clone())
                        .collect();
                    for p in paths {
                        self.stage(&p, false)
                    }
                }
            });
        });
        ui.add_space(6.);
        if let Some(repo) = self.repo() {
            let mut conflicts = Vec::new();
            if let Ok(index) = repo.index()
                && let Ok(entries) = index.conflicts()
            {
                for conflict in entries.flatten() {
                    if let Some(path) = conflict
                        .our
                        .or(conflict.their)
                        .or(conflict.ancestor)
                        .and_then(|entry| String::from_utf8(entry.path).ok())
                    {
                        conflicts.push(path);
                    }
                }
            }
            if !conflicts.is_empty() {
                egui::Frame::new()
                    .fill(Color32::from_rgb(57, 40, 34))
                    .stroke(Stroke::new(1., Color32::from_rgb(145, 94, 58)))
                    .corner_radius(8)
                    .inner_margin(8.)
                    .show(ui, |ui| {
                        ui.label(
                            RichText::new(format!("Merge conflicts · {}", conflicts.len()))
                                .strong()
                                .color(Color32::from_rgb(255, 195, 128)),
                        );
                        for path in conflicts {
                            ui.horizontal(|ui| {
                                if ui
                                    .selectable_label(
                                        self.selected_file.as_deref() == Some(&path),
                                        RichText::new(&path).color(TEXT),
                                    )
                                    .clicked()
                                {
                                    self.select_file(path.clone());
                                    self.edit_conflict(&path);
                                }
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    if ui
                                        .add_enabled(
                                            self.operation.is_none(),
                                            egui::Button::new("Mark resolved"),
                                        )
                                        .clicked()
                                    {
                                        self.resolve_conflict(&path, None);
                                    }
                                    if ui
                                        .add_enabled(
                                            self.operation.is_none(),
                                            egui::Button::new("Their version"),
                                        )
                                        .clicked()
                                    {
                                        self.resolve_conflict(&path, Some("--theirs"));
                                    }
                                    if ui
                                        .add_enabled(
                                            self.operation.is_none(),
                                            egui::Button::new("Our version"),
                                        )
                                        .clicked()
                                    {
                                        self.resolve_conflict(&path, Some("--ours"));
                                    }
                                });
                            });
                        }
                    });
                ui.add_space(6.);
            }
        }
        let mut save_conflict = None;
        let mut cancel_conflict_edit = false;
        if let Some((path, content)) = &mut self.conflict_draft {
            egui::Frame::new()
                .fill(PANEL)
                .stroke(Stroke::new(1., BORDER))
                .corner_radius(8)
                .inner_margin(8.)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(format!("Resolve · {path}"))
                                .strong()
                                .color(TEXT),
                        );
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui
                                .add_enabled(
                                    self.operation.is_none(),
                                    egui::Button::new("Save and mark resolved"),
                                )
                                .clicked()
                            {
                                save_conflict = Some((path.clone(), content.clone()));
                            }
                            if ui.button("Cancel edit").clicked() {
                                cancel_conflict_edit = true;
                            }
                        });
                    });
                    ui.add(
                        egui::TextEdit::multiline(content)
                            .code_editor()
                            .desired_rows(12)
                            .desired_width(f32::INFINITY),
                    );
                });
        }
        if cancel_conflict_edit {
            self.conflict_draft = None;
        }
        if let Some((path, content)) = save_conflict
            && let Some(root) = &self.path
        {
            match std::fs::write(root.join(&path), content) {
                Ok(()) => {
                    self.conflict_draft = None;
                    self.resolve_conflict(&path, None);
                }
                Err(error) => self.notice = format!("Could not save conflict file: {error}"),
            }
        }
        egui::Frame::new()
            .fill(PANEL)
            .stroke(Stroke::new(1., BORDER))
            .corner_radius(8)
            .inner_margin(8.)
            .show(ui, |ui| {
                if self.changes.is_empty() {
                    ui.label(RichText::new("Working tree clean").color(GREEN));
                }
                for c in self.changes.clone() {
                    ui.horizontal(|ui| {
                        if ui
                            .selectable_label(
                                self.selected_file.as_deref() == Some(&c.path),
                                RichText::new(format!(
                                    "{}  {}",
                                    if c.staged { "●" } else { "○" },
                                    c.path
                                ))
                                .color(TEXT),
                            )
                            .clicked()
                        {
                            self.select_file(c.path.clone())
                        }
                        ui.label(RichText::new(c.status).small().color(if c.staged {
                            GREEN
                        } else {
                            Color32::from_rgb(233, 174, 104)
                        }));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui.small_button(if c.staged { "−" } else { "+" }).clicked() {
                                self.stage(&c.path, c.staged)
                            }
                            if !c.staged && ui.small_button("Discard").clicked() {
                                self.pending_discard = Some(c.path.clone());
                            }
                        });
                    });
                }
            });
        egui::CollapsingHeader::new(format!("Stashes ({})", self.stashes.len()))
            .default_open(false)
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .max_height(150.)
                    .show(ui, |ui| {
                        for (index, message) in self.stashes.clone() {
                            let reference = format!("stash@{{{index}}}");
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(message).color(TEXT));
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    if ui.small_button("Drop").clicked() {
                                        self.git_action(
                                            "Drop stash",
                                            &["stash", "drop", &reference],
                                        );
                                    }
                                    if ui.small_button("Pop").clicked() {
                                        self.git_action(
                                            "Pop stash",
                                            &["stash", "pop", "--index", &reference],
                                        );
                                    }
                                    if ui.small_button("Apply").clicked() {
                                        self.git_action(
                                            "Apply stash",
                                            &["stash", "apply", "--index", &reference],
                                        );
                                    }
                                });
                            });
                        }
                        if self.stashes.is_empty() {
                            ui.label(RichText::new("No saved stashes").small().color(MUTED));
                        }
                    });
            });
        ui.add_space(10.);
        egui::Frame::new()
            .fill(PANEL)
            .stroke(Stroke::new(1., BORDER))
            .corner_radius(8)
            .inner_margin(8.)
            .show(ui, |ui| {
                ui.label(
                    RichText::new(self.selected_file.as_deref().unwrap_or("Diff preview"))
                        .strong()
                        .color(TEXT),
                );
                ui.separator();
                self.diff_panel(ui)
            });
        if let (Some(path), Some(repo)) = (self.selected_file.clone(), self.repo()) {
            let is_new = repo
                .status_file(Path::new(&path))
                .is_ok_and(|status| status.contains(Status::WT_NEW));
            if is_new {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Untracked file:").small().color(MUTED));
                    if ui
                        .add_enabled(self.operation.is_none(), egui::Button::new("Stage file"))
                        .clicked()
                    {
                        self.stage(&path, false);
                    }
                });
            } else {
                let hunks = unstaged_hunks(&repo, &path);
                if !hunks.is_empty() {
                    ui.horizontal_wrapped(|ui| {
                        if ui
                            .add_enabled(
                                self.operation.is_none(),
                                egui::Button::new("Edit patch to stage selected lines"),
                            )
                            .clicked()
                        {
                            self.edit_stage_patch(&path);
                        }
                        ui.label(RichText::new("Stage hunk:").small().color(MUTED));
                        for (index, (old_start, new_start, _, _)) in hunks.iter().enumerate() {
                            if ui
                                .add_enabled(
                                    self.operation.is_none(),
                                    egui::Button::new(format!("−{old_start} / +{new_start}")),
                                )
                                .clicked()
                            {
                                self.stage_hunk(&path, index);
                            }
                        }
                    });
                }
            }
        }
        let mut apply_patch = None;
        let mut cancel_patch = false;
        if let Some((path, patch)) = &mut self.patch_draft {
            egui::Frame::new()
                .fill(PANEL)
                .stroke(Stroke::new(1., BORDER))
                .corner_radius(8)
                .inner_margin(8.)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(format!("Partial staging · {path}"))
                                .strong()
                                .color(TEXT),
                        );
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui
                                .add_enabled(
                                    self.operation.is_none(),
                                    egui::Button::new("Stage edited patch"),
                                )
                                .clicked()
                            {
                                apply_patch = Some(patch.clone());
                            }
                            if ui.button("Cancel").clicked() {
                                cancel_patch = true;
                            }
                        });
                    });
                    ui.label(
                        RichText::new("Remove the + or − lines you do not want staged, then apply the remaining patch.")
                            .small()
                            .color(MUTED),
                    );
                    ui.add(
                        egui::TextEdit::multiline(patch)
                            .code_editor()
                            .desired_rows(12)
                            .desired_width(f32::INFINITY),
                    );
                });
        }
        if cancel_patch {
            self.patch_draft = None;
        }
        if let Some(patch) = apply_patch {
            self.apply_stage_patch(&patch);
        }
        ui.add_space(10.);
        egui::Frame::new()
            .fill(PANEL)
            .stroke(Stroke::new(1., BORDER))
            .corner_radius(8)
            .inner_margin(10.)
            .show(ui, |ui| {
                ui.label(
                    RichText::new("COMMIT MESSAGE")
                        .small()
                        .strong()
                        .color(MUTED),
                );
                ui.add_sized(
                    [ui.available_width(), 48.],
                    egui::TextEdit::multiline(&mut self.message).hint_text("Describe the changes…"),
                );
                ui.horizontal(|ui| {
                    let staged = self.changes.iter().filter(|c| c.staged).count();
                    if !self.commits.is_empty() {
                        ui.checkbox(&mut self.amend, "Amend last commit");
                    }
                    ui.label(
                        RichText::new(format!("{staged} staged"))
                            .small()
                            .color(MUTED),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui
                            .add_enabled(
                                staged > 0 || self.amend,
                                egui::Button::new("Commit changes  ↗")
                                    .fill(Color32::from_rgb(47, 111, 187)),
                            )
                            .clicked()
                        {
                            self.commit()
                        }
                    });
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
        let query = self.search.to_lowercase();
        let rows: Vec<_> = self
            .commits
            .iter()
            .filter(|c| {
                query.is_empty()
                    || c.message.to_lowercase().contains(&query)
                    || c.author.to_lowercase().contains(&query)
            })
            .cloned()
            .collect();
        if rows.is_empty() {
            self.empty(
                ui,
                if self.history_error.is_some() {
                    "History unavailable"
                } else if self.commits.is_empty() {
                    "No commits yet"
                } else {
                    "No matching commits"
                },
                self.history_error
                    .as_deref()
                    .unwrap_or(if self.commits.is_empty() {
                        "This repository has no reachable commit history yet."
                    } else {
                        "Adjust the commit or author filter to see more history."
                    }),
            );
            return;
        }
        let graph = graph_layout(&rows);
        let graph_width = graph
            .iter()
            .map(|row| row.before.len().max(row.after.len()))
            .max()
            .unwrap_or(1)
            .max(1) as f32
            * 16.
            + 8.;
        let mut picked = None;
        egui::Frame::new()
            .fill(PANEL)
            .stroke(Stroke::new(1., BORDER))
            .corner_radius(8)
            .inner_margin(8.)
            .show(ui, |ui| {
                egui::ScrollArea::vertical().show_rows(ui, 52., rows.len(), |ui, visible| {
                    for i in visible {
                        let c = &rows[i];
                        ui.horizontal(|ui| {
                            let (rect, _) = ui.allocate_exact_size(
                                egui::vec2(graph_width, 32.),
                                egui::Sense::hover(),
                            );
                            draw_graph_row(ui.painter(), rect, &graph[i]);
                            if ui
                                .selectable_label(
                                    self.selected_commit.as_deref() == Some(&c.id),
                                    RichText::new(&c.message).strong().color(TEXT),
                                )
                                .clicked()
                            {
                                picked = Some(c.id.clone())
                            }
                            ui.label(RichText::new(c.refs.join("  ")).small().color(GREEN));
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                ui.label(
                                    RichText::new(format!("{}  ·  {}", c.date, c.short))
                                        .small()
                                        .monospace()
                                        .color(MUTED),
                                )
                            });
                        });
                        ui.horizontal(|ui| {
                            ui.add_space(34.);
                            ui.label(RichText::new(&c.author).small().color(MUTED));
                        });
                    }
                })
            });
        if let Some(id) = picked {
            self.selected_commit = Some(id.clone());
            if let Some(r) = self.repo() {
                self.diff = diff_commit(&r, &id)
            }
        }
        if let Some(id) = self.selected_commit.clone() {
            ui.add_space(8.);
            egui::Frame::new()
                .fill(PANEL2)
                .stroke(Stroke::new(1., BORDER))
                .corner_radius(8)
                .inner_margin(8.)
                .show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            RichText::new(format!("Selected commit {}", &id[..7.min(id.len())]))
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
                            [150., 24.],
                            egui::TextEdit::singleline(&mut self.new_branch)
                                .hint_text("branch name"),
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
        }
        ui.add_space(10.);
        egui::Frame::new()
            .fill(PANEL)
            .stroke(Stroke::new(1., BORDER))
            .corner_radius(8)
            .inner_margin(8.)
            .show(ui, |ui| {
                ui.label(RichText::new("COMMIT DIFF").small().strong().color(MUTED));
                ui.separator();
                self.diff_panel(ui)
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
                    if ui.small_button("Delete tag").clicked() {
                        self.git_action("Delete tag", &["tag", "-d", &tag]);
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
                "Open a repository to browse tracked files.",
            );
            return;
        }
        let query = self.search.to_lowercase();
        let files: Vec<String> = self
            .tracked_files
            .iter()
            .filter(|path| query.is_empty() || path.to_lowercase().contains(&query))
            .cloned()
            .collect();
        ui.columns(2, |columns| {
            egui::Frame::new()
                .fill(PANEL)
                .stroke(Stroke::new(1., BORDER))
                .corner_radius(8)
                .inner_margin(6.)
                .show(&mut columns[0], |ui| {
                    ui.label(
                        RichText::new(format!("Tracked files · {}", files.len()))
                            .small()
                            .color(MUTED),
                    );
                    egui::ScrollArea::vertical().show_rows(ui, 24., files.len(), |ui, range| {
                        for index in range {
                            let path = &files[index];
                            if ui
                                .selectable_label(
                                    self.selected_file.as_deref() == Some(path),
                                    RichText::new(path).monospace().color(TEXT),
                                )
                                .clicked()
                            {
                                self.preview_file(path);
                            }
                        }
                    });
                });
            egui::Frame::new()
                .fill(PANEL)
                .stroke(Stroke::new(1., BORDER))
                .corner_radius(8)
                .inner_margin(8.)
                .show(&mut columns[1], |ui| {
                    ui.label(
                        RichText::new(self.selected_file.as_deref().unwrap_or("Select a file"))
                            .strong()
                            .color(TEXT),
                    );
                    ui.separator();
                    egui::ScrollArea::both().show(ui, |ui| {
                        for (line_number, line) in self.file_content.lines().enumerate() {
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new(format!("{:>4}", line_number + 1))
                                        .font(FontId::monospace(12.))
                                        .color(MUTED),
                                );
                                ui.label(
                                    RichText::new(line).font(FontId::monospace(12.)).color(TEXT),
                                );
                            });
                        }
                    });
                });
        });
    }
    fn diff_panel(&self, ui: &mut egui::Ui) {
        egui::ScrollArea::both().max_height(270.).show(ui, |ui| {
            if self.diff.is_empty() {
                ui.label(
                    RichText::new("Select a changed file or commit to preview its diff.")
                        .color(MUTED),
                );
            } else {
                for line in self.diff.lines().take(1500) {
                    let (color, bg) = if line.starts_with('+') && !line.starts_with("+++") {
                        (GREEN, Color32::from_rgb(25, 53, 45))
                    } else if line.starts_with('-') && !line.starts_with("---") {
                        (
                            Color32::from_rgb(242, 130, 137),
                            Color32::from_rgb(57, 37, 43),
                        )
                    } else if line.starts_with("@@") {
                        (BLUE, PANEL2)
                    } else {
                        (MUTED, PANEL)
                    };
                    ui.label(
                        RichText::new(line)
                            .font(FontId::monospace(12.))
                            .color(color)
                            .background_color(bg),
                    );
                }
            }
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
fn diff_file(r: &Repository, path: &str) -> String {
    let mut out = String::new();
    let mut staged = git2::DiffOptions::new();
    staged.pathspec(path);
    if let (Ok(head), Ok(index)) = (r.head().and_then(|h| h.peel_to_commit()), r.index())
        && let Ok(tree) = head.tree()
        && let Ok(diff) = r.diff_tree_to_index(Some(&tree), Some(&index), Some(&mut staged))
    {
        append_diff(&mut out, &diff);
    }
    let mut unstaged = git2::DiffOptions::new();
    unstaged
        .pathspec(path)
        .include_untracked(true)
        .recurse_untracked_dirs(true);
    if let Ok(diff) = r.diff_index_to_workdir(None, Some(&mut unstaged)) {
        append_diff(&mut out, &diff);
    }
    out
}
fn unstaged_patch(repo: &Repository, path: &str) -> Result<String, String> {
    let mut options = git2::DiffOptions::new();
    options.pathspec(path).context_lines(3);
    let diff = repo
        .diff_index_to_workdir(None, Some(&mut options))
        .map_err(|error| error.message().to_owned())?;
    let mut patch = String::new();
    diff.print(git2::DiffFormat::Patch, |_, _, line| {
        patch.push_str(std::str::from_utf8(line.content()).unwrap_or(""));
        true
    })
    .map_err(|error| error.message().to_owned())?;
    if patch.len() > 4 * 1024 * 1024 {
        return Err("Patch is larger than the 4 MB editor limit".into());
    }
    Ok(patch)
}
fn unstaged_hunks(repo: &Repository, path: &str) -> Vec<(u32, u32, u32, u32)> {
    let mut options = git2::DiffOptions::new();
    options
        .pathspec(path)
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
        out.push_str(std::str::from_utf8(l.content()).unwrap_or(""));
        true
    });
}
fn diff_commit(r: &Repository, id: &str) -> String {
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
    if let Ok(d) = r.diff_tree_to_tree(old.as_ref(), new.as_ref(), None) {
        let _ = d.print(git2::DiffFormat::Patch, |_, _, l| {
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
                    self.notice = format!("{label} failed: {error}")
                }
            }
        } else if self.operation.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
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
        ctx.set_visuals(egui::Visuals::dark());
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(BG).inner_margin(12.))
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(PANEL)
                    .stroke(Stroke::new(1., BORDER))
                    .corner_radius(10)
                    .inner_margin(10.)
                    .show(ui, |ui| self.header(ui));
                ui.add_space(10.);
                ui.horizontal(|ui| {
                    egui::Frame::new()
                        .fill(PANEL)
                        .stroke(Stroke::new(1., BORDER))
                        .corner_radius(10)
                        .inner_margin(12.)
                        .show(ui, |ui| self.sidebar(ui));
                    ui.add_space(10.);
                    egui::Frame::new()
                        .fill(PANEL2)
                        .stroke(Stroke::new(1., BORDER))
                        .corner_radius(10)
                        .inner_margin(14.)
                        .show(ui, |ui| self.content(ui));
                });
                ui.with_layout(Layout::bottom_up(Align::LEFT), |ui| {
                    ui.label(RichText::new(&self.notice).small().color(MUTED));
                });
            });
    }
}
fn main() -> eframe::Result<()> {
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
                active_path = storage
                    .get_string("active_repository")
                    .map(PathBuf::from)
                    .filter(|path| app.open_repos.contains(path));
            }
            if let Some(path) = active_path
                .or_else(|| app.open_repos.last().cloned())
                .or_else(|| app.recent_repos.first().cloned())
            {
                app.open(path);
            }
            Ok(Box::new(app))
        }),
    )
}
