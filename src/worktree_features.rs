use super::*;
#[cfg(unix)]
use std::os::unix::ffi::OsStringExt;
use std::{
    ffi::OsString,
    fs,
    path::{Component, Path},
};

const SHA256_UNSUPPORTED: &str = "System Git SHA256 repository is not supported by the libgit2 app";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct WorktreeRecord {
    path: PathBuf,
    head: Option<String>,
    branch: Option<String>,
    detached: bool,
    bare: bool,
    locked: bool,
    prunable: bool,
}

#[derive(Default)]
pub(super) struct WorktreeTools {
    path_input: String,
    branch_input: String,
    records: Vec<WorktreeRecord>,
    selected_path: Option<PathBuf>,
    pending_remove: Option<PathBuf>,
    prune_preview: Option<String>,
    confirm_prune: bool,
    loaded_repo: Option<PathBuf>,
    needs_refresh: bool,
    was_busy: bool,
}

impl GritApp {
    pub(super) fn worktree_tools_ui(&mut self, ui: &mut egui::Ui) {
        let Some(repo_path) = self.path.clone() else {
            ui.label("Open a Git repository to manage its linked worktrees.");
            return;
        };
        let Some(repo) = self.repo() else {
            if let Some(message) = sha256_repository_message(&repo_path) {
                ui.colored_label(Color32::YELLOW, message);
            }
            return;
        };
        let busy = self.operation.is_some();
        ui.separator();
        ui.heading("Linked worktrees");
        if repo.is_bare() {
            ui.label("Bare repository: history and linked worktree administration are available; this view does not provide working-tree file actions.");
        }
        if self.worktree_tools.loaded_repo.as_ref() != Some(&repo_path) {
            self.worktree_tools.loaded_repo = Some(repo_path.clone());
            self.worktree_tools.needs_refresh = true;
        }
        if self.worktree_tools.was_busy && !busy {
            self.worktree_tools.needs_refresh = true;
        }
        self.worktree_tools.was_busy = busy;
        if self.worktree_tools.needs_refresh && !busy {
            self.worktree_tools.records = list_worktrees(&repo_path).unwrap_or_else(|error| {
                self.notice = format!("Could not list worktrees: {error}");
                Vec::new()
            });
            self.worktree_tools.needs_refresh = false;
        }
        ui.horizontal(|ui| {
            if ui
                .add_enabled(!busy, egui::Button::new("Refresh worktrees"))
                .clicked()
            {
                match list_worktrees(&repo_path) {
                    Ok(records) => self.worktree_tools.records = records,
                    Err(error) => self.notice = format!("Could not list worktrees: {error}"),
                }
            }
            if ui
                .add_enabled(!busy, egui::Button::new("Open selected worktree"))
                .clicked()
                && let Some(path) = self.worktree_tools.selected_path.clone()
                && self
                    .worktree_tools
                    .records
                    .iter()
                    .any(|record| record.path == path && !record.bare && !record.prunable)
            {
                self.open(path);
            }
        });

        let current = repo.workdir().unwrap_or(&repo_path).to_path_buf();
        egui::ScrollArea::vertical()
            .max_height(180.)
            .show(ui, |ui| {
                for record in self.worktree_tools.records.clone() {
                    let current_record = record.path == current;
                    let details = if record.bare {
                        "bare".to_owned()
                    } else if record.detached {
                        format!(
                            "detached · {}",
                            record.head.as_deref().unwrap_or("unknown HEAD")
                        )
                    } else {
                        record
                            .branch
                            .clone()
                            .unwrap_or_else(|| "branch unavailable".into())
                    };
                    let details = format!(
                        "{details}{}{}",
                        if record.locked { " · locked" } else { "" },
                        if record.prunable { " · prunable" } else { "" }
                    );
                    ui.horizontal(|ui| {
                        let selected =
                            self.worktree_tools.selected_path.as_ref() == Some(&record.path);
                        if ui
                            .selectable_label(
                                selected,
                                format!("{}   ({details})", record.path.display()),
                            )
                            .clicked()
                        {
                            self.worktree_tools.selected_path = Some(record.path.clone());
                        }
                        if !record.bare
                            && !current_record
                            && !record.prunable
                            && ui
                                .add_enabled(!busy, egui::Button::new("Remove…"))
                                .clicked()
                        {
                            self.worktree_tools.pending_remove = Some(record.path.clone());
                        }
                    });
                }
            });

        if let Some(path) = self.worktree_tools.pending_remove.clone() {
            ui.colored_label(Color32::YELLOW, format!("Remove linked worktree {}? Git will refuse dirty worktrees; grit never uses --force.", path.display()));
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(!busy, egui::Button::new("Confirm remove worktree"))
                    .clicked()
                {
                    let target = path.clone();
                    self.start_worktree_command("Remove worktree", move |root| {
                        remove_worktree(&root, &target)
                    });
                    self.worktree_tools.pending_remove = None;
                    self.worktree_tools.needs_refresh = true;
                }
                if ui.button("Cancel").clicked() {
                    self.worktree_tools.pending_remove = None;
                }
            });
        }

        ui.separator();
        ui.label("Create linked worktree with a new branch");
        ui.horizontal(|ui| {
            ui.add_sized(
                [250., 24.],
                egui::TextEdit::singleline(&mut self.worktree_tools.path_input)
                    .hint_text("Worktree path"),
            );
            ui.add_sized(
                [170., 24.],
                egui::TextEdit::singleline(&mut self.worktree_tools.branch_input)
                    .hint_text("New branch"),
            );
        });
        let valid = !self.worktree_tools.path_input.trim().is_empty()
            && valid_branch_name(&repo, self.worktree_tools.branch_input.trim()).is_ok()
            && prepare_worktree_path(&repo_path, self.worktree_tools.path_input.trim()).is_ok();
        if ui
            .add_enabled(!busy && valid, egui::Button::new("Create worktree"))
            .clicked()
        {
            let path = self.worktree_tools.path_input.trim().to_owned();
            let branch = self.worktree_tools.branch_input.trim().to_owned();
            let target = match self.selected_commit.as_deref() {
                Some(selected) => resolve_commit(&repo, selected),
                None => repo
                    .head()
                    .and_then(|head| head.peel_to_commit())
                    .map(|commit| commit.id().to_string())
                    .map_err(|e| e.to_string()),
            };
            match target {
                Ok(target) => {
                    self.start_worktree_command("Create worktree", move |root| {
                        create_worktree(&root, &path, &branch, &target)
                    });
                    self.worktree_tools.needs_refresh = true;
                }
                Err(error) => {
                    self.notice = format!("Could not resolve worktree start commit: {error}")
                }
            }
        }
        let start = self.selected_commit.as_deref().unwrap_or("HEAD");
        ui.label(format!("New branch starts at selected commit {start} (or current HEAD when no commit is selected)."));

        ui.separator();
        ui.label("Prune stale worktree metadata");
        if ui
            .add_enabled(!busy, egui::Button::new("Preview prune"))
            .clicked()
        {
            match prune_preview(&repo_path) {
                Ok(preview) => {
                    self.worktree_tools.prune_preview = Some(preview);
                    self.worktree_tools.confirm_prune = false;
                }
                Err(error) => self.notice = format!("Could not preview prune: {error}"),
            }
        }
        if let Some(preview) = self.worktree_tools.prune_preview.clone() {
            ui.label(if preview.trim().is_empty() {
                "No stale worktree metadata found."
            } else {
                &preview
            });
            if !preview.trim().is_empty()
                && !self.worktree_tools.confirm_prune
                && ui
                    .add_enabled(!busy, egui::Button::new("Review prune"))
                    .clicked()
            {
                self.worktree_tools.confirm_prune = true;
            }
            if self.worktree_tools.confirm_prune {
                ui.label("Confirm removal of the stale metadata shown above?");
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(!busy, egui::Button::new("Confirm prune"))
                        .clicked()
                    {
                        self.start_worktree_command("Prune worktrees", move |root| {
                            prune_worktrees(&root)
                        });
                        self.worktree_tools.confirm_prune = false;
                        self.worktree_tools.prune_preview = None;
                        self.worktree_tools.needs_refresh = true;
                    }
                    if ui.button("Cancel").clicked() {
                        self.worktree_tools.confirm_prune = false;
                    }
                });
            }
        }
    }

    fn start_worktree_command<F>(&mut self, label: &'static str, work: F)
    where
        F: FnOnce(PathBuf) -> Result<String, String> + Send + 'static,
    {
        if self.operation.is_some() {
            return;
        }
        let Some(root) = self.path.clone() else {
            self.notice = "Open a repository first".into();
            return;
        };
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(work(root));
        });
        self.notice = format!("{label} running…");
        self.operation = Some((label.into(), rx));
    }
}

fn git_output(root: &Path, args: &[OsString]) -> Result<Vec<u8>, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .map_err(|e| e.to_string())?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        Err(if stderr.is_empty() {
            format!("git exited with {}", output.status)
        } else {
            stderr
        })
    }
}

fn os_arg(value: &Path) -> OsString {
    value.as_os_str().to_owned()
}
fn str_arg(value: &str) -> OsString {
    OsString::from(value)
}

fn git_list_worktrees(root: &Path) -> Result<Vec<u8>, String> {
    git_output(
        root,
        &[
            str_arg("worktree"),
            str_arg("list"),
            str_arg("--porcelain"),
            str_arg("-z"),
        ],
    )
}

fn path_from_git_bytes(bytes: &[u8]) -> PathBuf {
    #[cfg(unix)]
    {
        PathBuf::from(OsString::from_vec(bytes.to_vec()))
    }
    #[cfg(not(unix))]
    {
        PathBuf::from(String::from_utf8_lossy(bytes).into_owned())
    }
}

fn parse_worktree_list(bytes: &[u8]) -> Result<Vec<WorktreeRecord>, String> {
    let mut records = Vec::new();
    let mut current = WorktreeRecord::default();
    let mut have_record = false;
    for field in bytes.split(|byte| *byte == 0) {
        if field.is_empty() {
            if have_record {
                records.push(current);
                current = WorktreeRecord::default();
                have_record = false;
            }
            continue;
        }
        have_record = true;
        if let Some(path) = field.strip_prefix(b"worktree ") {
            current.path = path_from_git_bytes(path);
        } else if let Some(head) = field.strip_prefix(b"HEAD ") {
            current.head = Some(
                std::str::from_utf8(head)
                    .map_err(|_| "Worktree HEAD is not valid UTF-8")?
                    .to_owned(),
            );
        } else if let Some(branch) = field.strip_prefix(b"branch ") {
            current.branch = Some(
                std::str::from_utf8(branch)
                    .map_err(|_| "Worktree branch is not valid UTF-8")?
                    .to_owned(),
            );
        } else if field == b"detached" {
            current.detached = true;
        } else if field == b"bare" {
            current.bare = true;
        } else if field == b"locked" || field.starts_with(b"locked ") {
            current.locked = true;
        } else if field == b"prunable" || field.starts_with(b"prunable ") {
            current.prunable = true;
        }
    }
    if have_record {
        records.push(current);
    }
    if records
        .iter()
        .any(|record| record.path.as_os_str().is_empty())
    {
        return Err("Git returned a worktree entry without a path".into());
    }
    Ok(records)
}

fn list_worktrees(root: &Path) -> Result<Vec<WorktreeRecord>, String> {
    parse_worktree_list(&git_list_worktrees(root)?)
}

pub(super) fn sha256_repository_message(path: &Path) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["rev-parse", "--show-object-format"])
        .output()
        .ok()?;
    (output.status.success() && String::from_utf8_lossy(&output.stdout).trim() == "sha256")
        .then(|| SHA256_UNSUPPORTED.to_owned())
}

fn resolve_commit(repo: &Repository, spec: &str) -> Result<String, String> {
    repo.revparse_single(spec)
        .and_then(|object| object.peel_to_commit())
        .map(|commit| commit.id().to_string())
        .map_err(|error| error.to_string())
}

fn valid_branch_name(repo: &Repository, branch: &str) -> Result<(), String> {
    if branch.trim().is_empty() || branch.contains(['\n', '\r', '\0']) {
        return Err("Enter a valid new branch name".into());
    }
    let full = format!("refs/heads/{branch}");
    if !git2::Reference::is_valid_name(&full) {
        return Err("Branch name is not a valid Git reference".into());
    }
    if repo.find_branch(branch, BranchType::Local).is_ok() {
        return Err("That local branch already exists".into());
    }
    Ok(())
}

fn prepare_worktree_path(repo_root: &Path, input: &str) -> Result<PathBuf, String> {
    if input.trim().is_empty() || input.contains(['\n', '\r', '\0']) {
        return Err("Enter one worktree path".into());
    }
    let supplied = Path::new(input);
    if supplied
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err("Worktree path cannot contain `..`".into());
    }
    let candidate = if supplied.is_absolute() {
        supplied.to_path_buf()
    } else {
        repo_root.join(supplied)
    };
    if fs::symlink_metadata(&candidate).is_ok() {
        return Err("Worktree destination already exists".into());
    }
    let parent = candidate
        .parent()
        .ok_or("Worktree destination has no parent")?
        .canonicalize()
        .map_err(|e| format!("Could not resolve worktree parent: {e}"))?;
    let name = candidate
        .file_name()
        .ok_or("Worktree destination needs a directory name")?;
    Ok(parent.join(name))
}

fn create_worktree(
    repo_root: &Path,
    destination: &str,
    branch: &str,
    target: &str,
) -> Result<String, String> {
    let repo = Repository::open(repo_root).map_err(|error| {
        sha256_repository_message(repo_root).unwrap_or_else(|| error.to_string())
    })?;
    valid_branch_name(&repo, branch)?;
    let target = resolve_commit(&repo, target)?;
    let destination = prepare_worktree_path(repo_root, destination)?;
    let output = git_output(
        repo_root,
        &[
            str_arg("worktree"),
            str_arg("add"),
            str_arg("-b"),
            str_arg(branch),
            str_arg("--"),
            os_arg(&destination),
            str_arg(&target),
        ],
    )?;
    Ok(String::from_utf8_lossy(&output).trim().to_owned())
}

fn remove_worktree(repo_root: &Path, target: &Path) -> Result<String, String> {
    let repo = Repository::open(repo_root).map_err(|error| {
        sha256_repository_message(repo_root).unwrap_or_else(|| error.to_string())
    })?;
    let target = target
        .canonicalize()
        .map_err(|e| format!("Could not resolve selected worktree: {e}"))?;
    let current = repo
        .workdir()
        .unwrap_or(repo_root)
        .canonicalize()
        .unwrap_or_else(|_| repo.workdir().unwrap_or(repo_root).to_path_buf());
    if target == current {
        return Err("Cannot remove the active repository worktree".into());
    }
    let exists = list_worktrees(repo_root)?.iter().any(|record| {
        !record.bare
            && !record.prunable
            && record.path.canonicalize().ok().as_ref() == Some(&target)
    });
    if !exists {
        return Err("Selected path is not an existing linked worktree".into());
    }
    let output = git_output(
        repo_root,
        &[
            str_arg("worktree"),
            str_arg("remove"),
            str_arg("--"),
            os_arg(&target),
        ],
    )?;
    Ok(String::from_utf8_lossy(&output).trim().to_owned())
}

fn prune_preview(repo_root: &Path) -> Result<String, String> {
    let output = git_output(
        repo_root,
        &[
            str_arg("worktree"),
            str_arg("prune"),
            str_arg("--dry-run"),
            str_arg("--verbose"),
        ],
    )?;
    Ok(String::from_utf8_lossy(&output).trim().to_owned())
}

fn prune_worktrees(repo_root: &Path) -> Result<String, String> {
    let output = git_output(
        repo_root,
        &[str_arg("worktree"), str_arg("prune"), str_arg("--verbose")],
    )?;
    Ok(String::from_utf8_lossy(&output).trim().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    struct TestRepo(PathBuf);
    impl TestRepo {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "grit-worktree-{name}-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir_all(&path).unwrap();
            let output = Command::new("git")
                .args(["init", "-q", path.to_str().unwrap()])
                .output()
                .unwrap();
            assert!(output.status.success());
            let repo = Self(path);
            repo.git(&["config", "user.name", "Worktree Test"]);
            repo.git(&["config", "user.email", "worktree@example.test"]);
            fs::write(repo.0.join("file.txt"), "seed\n").unwrap();
            repo.git(&["add", "file.txt"]);
            repo.git(&["commit", "-qm", "seed"]);
            repo
        }
        fn git(&self, args: &[&str]) -> String {
            let output = Command::new("git")
                .arg("-C")
                .arg(&self.0)
                .args(args)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8_lossy(&output.stdout).trim().to_owned()
        }
    }
    impl Drop for TestRepo {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn parses_nul_delimited_records_with_spaces_unicode_detached_bare_and_flags() {
        let fixture = b"worktree /tmp/wt with \xc3\xbc\0HEAD abc123\0branch refs/heads/feature\0\0worktree /tmp/detached \xc3\xa9\0HEAD def456\0detached\0locked busy\0prunable stale\0\0worktree /tmp/repo.git\0bare\0\0";
        let parsed = parse_worktree_list(fixture).unwrap();
        assert_eq!(parsed.len(), 3);
        assert_eq!(parsed[0].path, PathBuf::from("/tmp/wt with ü"));
        assert_eq!(parsed[0].branch.as_deref(), Some("refs/heads/feature"));
        assert!(parsed[1].detached && parsed[1].locked && parsed[1].prunable);
        assert_eq!(parsed[1].path, PathBuf::from("/tmp/detached é"));
        assert!(parsed[2].bare);
    }

    #[test]
    fn creates_lists_opens_detached_and_removes_real_worktrees_refusing_dirty_removal() {
        let repo = TestRepo::new("spaces ü");
        let linked = std::env::temp_dir().join(format!("linked worktree ü {}", std::process::id()));
        let _ = fs::remove_dir_all(&linked);
        let head = repo.git(&["rev-parse", "HEAD"]);
        create_worktree(&repo.0, linked.to_str().unwrap(), "feature/unicode", &head).unwrap();
        let records = list_worktrees(&repo.0).unwrap();
        let record = records.iter().find(|record| record.path == linked).unwrap();
        assert_eq!(record.branch.as_deref(), Some("refs/heads/feature/unicode"));
        assert_eq!(
            Repository::open(&linked)
                .unwrap()
                .head()
                .unwrap()
                .peel_to_commit()
                .unwrap()
                .id()
                .to_string(),
            head
        );

        fs::write(linked.join("untracked dirty ü.txt"), "dirty\n").unwrap();
        assert!(remove_worktree(&repo.0, &linked).is_err());
        assert!(linked.exists(), "dirty worktree must be refused");
        fs::remove_file(linked.join("untracked dirty ü.txt")).unwrap();
        remove_worktree(&repo.0, &linked).unwrap();
        assert!(!linked.exists());
        assert!(
            !list_worktrees(&repo.0)
                .unwrap()
                .iter()
                .any(|record| record.path == linked)
        );

        let detached =
            std::env::temp_dir().join(format!("detached worktree {}", std::process::id()));
        let _ = fs::remove_dir_all(&detached);
        repo.git(&[
            "worktree",
            "add",
            "--detach",
            detached.to_str().unwrap(),
            &head,
        ]);
        let detached_record = list_worktrees(&repo.0)
            .unwrap()
            .into_iter()
            .find(|record| record.path == detached)
            .unwrap();
        assert!(detached_record.detached);
        remove_worktree(&repo.0, &detached).unwrap();
    }

    #[test]
    fn bare_repository_keeps_history_and_worktree_administration_available() {
        let repo = TestRepo::new("bare source");
        let bare = repo.0.with_extension("bare.git");
        let output = Command::new("git")
            .args(["clone", "-q", "--bare"])
            .arg(&repo.0)
            .arg(&bare)
            .output()
            .unwrap();
        assert!(output.status.success());
        let opened = Repository::open(&bare).unwrap();
        assert!(opened.is_bare());
        assert!(
            opened.head().unwrap().peel_to_commit().is_ok(),
            "bare repositories still expose history"
        );
        let worktrees = list_worktrees(&bare).unwrap();
        assert!(worktrees.iter().any(|record| record.bare));
        let _ = fs::remove_dir_all(bare);
    }

    #[test]
    fn detects_system_git_sha256_repository_with_actionable_message() {
        let path = std::env::temp_dir().join(format!(
            "grit-sha256-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&path).unwrap();
        let output = Command::new("git")
            .args(["init", "-q", "--object-format=sha256"])
            .arg(&path)
            .output()
            .unwrap();
        if output.status.success() {
            assert_eq!(
                sha256_repository_message(&path).as_deref(),
                Some(SHA256_UNSUPPORTED)
            );
        }
        let _ = fs::remove_dir_all(path);
    }
}
