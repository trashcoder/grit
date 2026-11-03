use super::*;
use std::process::Command;

#[derive(Clone, Debug, Default)]
struct StashPushOptions {
    name: String,
    include_untracked: bool,
    keep_index: bool,
    only_staged: bool,
}

#[derive(Default)]
pub(crate) struct StashTools {
    push: StashPushOptions,
    restore_index: bool,
    selected_stash: Option<SelectedStash>,
    pending_drop: Option<SelectedStash>,
    staged_supported: Option<bool>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SelectedStash {
    repo_path: PathBuf,
    index: usize,
    oid: String,
}

fn git_version() -> Result<(u32, u32, u32), String> {
    let output = Command::new("git")
        .arg("--version")
        .output()
        .map_err(|error| format!("Could not check Git version: {error}"))?;
    if !output.status.success() {
        return Err("Could not check Git version".into());
    }
    let version = String::from_utf8_lossy(&output.stdout);
    let value = version
        .split_whitespace()
        .nth(2)
        .ok_or_else(|| format!("Unrecognized Git version: {}", version.trim()))?;
    let mut parts = value.split('.').map(|part| {
        part.chars()
            .take_while(|character| character.is_ascii_digit())
            .collect::<String>()
            .parse::<u32>()
            .unwrap_or(0)
    });
    let major = parts.next().unwrap_or(0);
    let minor = parts.next().unwrap_or(0);
    let patch = parts.next().unwrap_or(0);
    Ok((major, minor, patch))
}

fn supports_staged_stash() -> Result<bool, String> {
    Ok(git_version()? >= (2, 35, 0))
}

fn build_push_args(
    options: &StashPushOptions,
    staged_supported: bool,
) -> Result<Vec<String>, String> {
    if options.name.trim().is_empty() {
        return Err("Enter a stash name.".into());
    }
    if options.name.contains('\0') || options.name.contains('\n') || options.name.contains('\r') {
        return Err("Stash names cannot contain NUL or line breaks.".into());
    }
    if options.only_staged && !staged_supported {
        return Err("Stashing only staged changes requires Git 2.35 or newer.".into());
    }
    if options.only_staged && (options.include_untracked || options.keep_index) {
        return Err(
            "Only staged changes cannot be combined with include untracked or keep index.".into(),
        );
    }
    let mut args = vec!["stash".into(), "push".into()];
    if options.only_staged {
        args.push("--staged".into());
    }
    if options.keep_index {
        args.push("--keep-index".into());
    }
    if options.include_untracked {
        args.push("--include-untracked".into());
    }
    args.push("--message".into());
    args.push(options.name.trim().to_owned());
    Ok(args)
}

fn restore_args(action: &str, index: usize, restore_index: bool) -> Vec<String> {
    let mut args = vec!["stash".into(), action.into()];
    if restore_index {
        args.push("--index".into());
    }
    args.push(format!("stash@{{{index}}}"));
    args
}

fn stash_oid(repo: &Repository, index: usize) -> Result<String, String> {
    repo.revparse_single(&format!("stash@{{{index}}}"))
        .and_then(|object| object.peel_to_commit())
        .map(|commit| commit.id().to_string())
        .map_err(|error| format!("Could not resolve stash@{{{index}}}: {error}"))
}

fn validated_stash_action(
    repo: &Repository,
    repo_path: &Path,
    selected: &SelectedStash,
    action: &str,
    restore_index: bool,
) -> Result<Vec<String>, String> {
    if selected.repo_path != repo_path || stash_oid(repo, selected.index)? != selected.oid {
        return Err(
            "This stash entry changed since selection. Select it again before continuing.".into(),
        );
    }
    Ok(restore_args(action, selected.index, restore_index))
}

fn validated_drop_action(
    repo: &Repository,
    repo_path: &Path,
    selected: &SelectedStash,
) -> Result<Vec<String>, String> {
    if selected.repo_path != repo_path || stash_oid(repo, selected.index)? != selected.oid {
        return Err(
            "This stash entry changed since selection. Select it again before continuing.".into(),
        );
    }
    Ok(vec![
        "stash".into(),
        "drop".into(),
        format!("stash@{{{}}}", selected.index),
    ])
}

fn effects(options: &StashPushOptions) -> &'static str {
    if options.only_staged {
        "Creates a named stash from staged changes only. Unstaged and untracked files remain as they are."
    } else if options.keep_index {
        if options.include_untracked {
            "Stashes tracked changes and untracked files; staged changes remain in the index and working tree."
        } else {
            "Stashes tracked changes; staged changes remain in the index and working tree. Untracked files are left alone."
        }
    } else if options.include_untracked {
        "Stashes tracked changes and untracked files, then restores the tracked working tree and index to HEAD."
    } else {
        "Stashes tracked changes only. Untracked files are left alone; the tracked working tree and index are restored to HEAD."
    }
}

impl GritApp {
    pub(crate) fn stash_options_ui(&mut self, ui: &mut egui::Ui) {
        ui.heading("Create named stash");
        let busy = self.operation.is_some();
        if self.stash_tools.staged_supported.is_none() {
            self.stash_tools.staged_supported = Some(supports_staged_stash().unwrap_or(false));
        }
        let staged_supported = self.stash_tools.staged_supported.unwrap_or(false);
        ui.horizontal(|ui| {
            ui.label("Name");
            ui.add_sized(
                [ui.available_width(), 24.],
                egui::TextEdit::singleline(&mut self.stash_tools.push.name)
                    .hint_text("e.g. before-rebase"),
            );
        });
        ui.checkbox(
            &mut self.stash_tools.push.include_untracked,
            "Include untracked files",
        );
        ui.checkbox(
            &mut self.stash_tools.push.keep_index,
            "Keep staged changes in index",
        );
        ui.add_enabled_ui(staged_supported, |ui| {
            ui.checkbox(
                &mut self.stash_tools.push.only_staged,
                "Stash only staged changes (Git 2.35+)",
            );
        });
        if !staged_supported {
            ui.label("Only staged requires Git 2.35 or newer.");
        }
        if self.stash_tools.push.only_staged {
            ui.label("Git may reject this if unstaged edits overlap staged changes in a file.");
        }
        let preview = build_push_args(&self.stash_tools.push, staged_supported);
        match &preview {
            Ok(_) => {
                let enabled_options = [
                    self.stash_tools
                        .push
                        .include_untracked
                        .then_some("untracked files"),
                    self.stash_tools
                        .push
                        .keep_index
                        .then_some("keep staged changes in the index"),
                    self.stash_tools
                        .push
                        .only_staged
                        .then_some("staged changes only"),
                ]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>();
                ui.label(format!("Name: {}", self.stash_tools.push.name.trim()));
                ui.label(if enabled_options.is_empty() {
                    "Options: tracked changes; untracked files are left out.".to_owned()
                } else {
                    format!("Options: {}", enabled_options.join(", "))
                });
                ui.label(effects(&self.stash_tools.push));
            }
            Err(error) => {
                ui.colored_label(Color32::YELLOW, error);
            }
        }
        ui.add_enabled_ui(!busy, |ui| {
            if ui
                .add_enabled(preview.is_ok(), egui::Button::new("Create stash"))
                .clicked()
                && let Ok(args) = preview
            {
                self.start_git_command("Create stash", args);
            }
        });

        ui.separator();
        ui.heading("Existing stashes");
        if self.stashes.is_empty() {
            ui.label("No stashes in this repository.");
        } else {
            egui::ComboBox::from_id_salt("stash_restore_selection")
                .selected_text(
                    self.stash_tools
                        .selected_stash
                        .as_ref()
                        .and_then(|stash| {
                            self.stashes
                                .iter()
                                .find(|(index, _)| *index == stash.index)
                                .map(|(index, message)| format!("stash@{{{index}}}: {message}"))
                        })
                        .unwrap_or_else(|| "Select a stash".into()),
                )
                .show_ui(ui, |ui| {
                    for (index, message) in self.stashes.clone() {
                        let is_selected = self
                            .stash_tools
                            .selected_stash
                            .as_ref()
                            .is_some_and(|stash| stash.index == index);
                        if ui
                            .selectable_label(is_selected, format!("stash@{{{index}}}: {message}"))
                            .clicked()
                            && let (Some(repo), Some(repo_path)) = (self.repo(), self.path.clone())
                        {
                            match stash_oid(&repo, index) {
                                Ok(oid) => {
                                    self.stash_tools.selected_stash = Some(SelectedStash {
                                        repo_path,
                                        index,
                                        oid,
                                    })
                                }
                                Err(error) => self.notice = error,
                            }
                        }
                    }
                });
            ui.checkbox(
                &mut self.stash_tools.restore_index,
                "Restore staged state when applying",
            );
            let selected = self.stash_tools.selected_stash.clone();
            let selected_repo = self.repo();
            let repo_path = self.path.clone();
            ui.horizontal(|ui| {
                let enabled = !busy && selected.is_some();
                if ui
                    .add_enabled(enabled, egui::Button::new("Apply"))
                    .clicked()
                    && let (Some(repo), Some(path), Some(stash)) = (
                        selected_repo.as_ref(),
                        repo_path.as_deref(),
                        selected.as_ref(),
                    )
                {
                    match validated_stash_action(
                        repo,
                        path,
                        stash,
                        "apply",
                        self.stash_tools.restore_index,
                    ) {
                        Ok(args) => self.start_git_command("Apply stash", args),
                        Err(error) => self.notice = error,
                    }
                }
                if ui.add_enabled(enabled, egui::Button::new("Pop")).clicked()
                    && let (Some(repo), Some(path), Some(stash)) = (
                        selected_repo.as_ref(),
                        repo_path.as_deref(),
                        selected.as_ref(),
                    )
                {
                    match validated_stash_action(
                        repo,
                        path,
                        stash,
                        "pop",
                        self.stash_tools.restore_index,
                    ) {
                        Ok(args) => self.start_git_command("Pop stash", args),
                        Err(error) => self.notice = error,
                    }
                }
                if ui
                    .add_enabled(enabled, egui::Button::new("Drop…"))
                    .clicked()
                {
                    self.stash_tools.pending_drop = selected.clone();
                }
            });
            if let Some(stash) = selected.as_ref() {
                ui.label(format!(
                    "Restore preview: apply stash@{{{}}}{}.",
                    stash.index,
                    if self.stash_tools.restore_index {
                        " and restore its staged state to the index"
                    } else {
                        " to the working tree, preserving the current index"
                    }
                ));
                ui.label(if self.stash_tools.restore_index {
                    "Apply changes and attempt to restore the stash's staged state in the index."
                } else {
                    "Apply changes to the working tree; the current index is not restored from the stash."
                });
            }
        }
        if let Some(pending) = self.stash_tools.pending_drop.clone() {
            egui::Window::new("Drop stash?")
                .collapsible(false)
                .resizable(false)
                .show(ui.ctx(), |ui| {
                    ui.label(format!(
                        "Permanently drop stash@{{{}}} ({})?",
                        pending.index, pending.oid
                    ));
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(!busy, egui::Button::new("Confirm drop"))
                            .clicked()
                        {
                            if let (Some(repo), Some(path)) = (self.repo(), self.path.clone()) {
                                match validated_drop_action(&repo, &path, &pending) {
                                    Ok(args) => self.start_git_command("Drop stash", args),
                                    Err(error) => self.notice = error,
                                }
                            }
                            self.stash_tools.pending_drop = None;
                        }
                        if ui.button("Cancel").clicked() {
                            self.stash_tools.pending_drop = None;
                        }
                    });
                });
        }
    }
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
        fn new() -> Self {
            let id = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!("grit-stash-{id}"));
            fs::create_dir_all(&path).unwrap();
            let repo = Self(path);
            repo.git(&["init", "-q"]);
            repo.git(&["config", "user.name", "Stash Test"]);
            repo.git(&["config", "user.email", "stash@example.test"]);
            fs::write(repo.0.join("tracked.txt"), "base\n").unwrap();
            fs::write(repo.0.join("other.txt"), "other base\n").unwrap();
            repo.git(&["add", "tracked.txt"]);
            repo.git(&["add", "other.txt"]);
            repo.git(&["commit", "-qm", "base"]);
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
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8_lossy(&output.stdout).trim().to_owned()
        }
        fn run(&self, args: &[String]) -> String {
            let output = Command::new("git")
                .arg("-C")
                .arg(&self.0)
                .args(args)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
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

    fn opts(name: &str) -> StashPushOptions {
        StashPushOptions {
            name: name.into(),
            ..Default::default()
        }
    }

    #[test]
    fn unusual_stash_name_is_one_literal_argument() {
        let repo = TestRepo::new();
        fs::write(repo.0.join("tracked.txt"), "changed\n").unwrap();
        let name = "release; $(echo unsafe) ' quoted";
        let args = build_push_args(&opts(name), true).unwrap();
        repo.run(&args);
        assert!(repo.git(&["stash", "list", "--format=%s"]).ends_with(name));
        assert_eq!(args.last().unwrap(), name);
    }

    #[test]
    fn untracked_is_not_included_unless_requested() {
        let repo = TestRepo::new();
        fs::write(repo.0.join("loose.txt"), "loose\n").unwrap();
        repo.run(&build_push_args(&opts("tracked only"), true).unwrap());
        assert!(repo.git(&["stash", "list"]).is_empty());
        let mut options = opts("with untracked");
        options.include_untracked = true;
        repo.run(&build_push_args(&options, true).unwrap());
        assert_eq!(
            repo.git(&["ls-tree", "-r", "--name-only", "stash@{0}^3"]),
            "loose.txt"
        );
    }

    #[test]
    fn keep_index_preserves_staged_content() {
        let repo = TestRepo::new();
        fs::write(repo.0.join("tracked.txt"), "staged\n").unwrap();
        repo.git(&["add", "tracked.txt"]);
        fs::write(repo.0.join("tracked.txt"), "staged plus unstaged\n").unwrap();
        let mut options = opts("keep index");
        options.keep_index = true;
        repo.run(&build_push_args(&options, true).unwrap());
        assert_eq!(
            fs::read_to_string(repo.0.join("tracked.txt")).unwrap(),
            "staged\n"
        );
        assert_eq!(repo.git(&["show", ":tracked.txt"]), "staged");
    }

    #[test]
    fn staged_only_checks_version_and_rejects_conflicting_flags() {
        let mut options = opts("only staged");
        options.only_staged = true;
        assert!(build_push_args(&options, false).is_err());
        assert!(
            build_push_args(&options, true)
                .unwrap()
                .contains(&"--staged".into())
        );
        options.keep_index = true;
        assert!(build_push_args(&options, true).is_err());
    }

    #[test]
    fn staged_only_stashes_index_and_leaves_unstaged_worktree_content() {
        if !supports_staged_stash().unwrap_or(false) {
            return;
        }
        let repo = TestRepo::new();
        fs::write(repo.0.join("tracked.txt"), "staged\n").unwrap();
        repo.git(&["add", "tracked.txt"]);
        fs::write(repo.0.join("other.txt"), "unstaged on top\n").unwrap();
        let mut options = opts("only staged");
        options.only_staged = true;
        repo.run(&build_push_args(&options, true).unwrap());
        assert_eq!(repo.git(&["show", "stash@{0}:tracked.txt"]), "staged");
        assert_eq!(
            fs::read_to_string(repo.0.join("other.txt")).unwrap(),
            "unstaged on top\n"
        );
    }

    #[test]
    fn apply_index_option_restores_staged_state() {
        let repo = TestRepo::new();
        fs::write(repo.0.join("tracked.txt"), "staged\n").unwrap();
        repo.git(&["add", "tracked.txt"]);
        repo.run(&build_push_args(&opts("index restore"), true).unwrap());
        let args = restore_args("apply", 0, true);
        assert_eq!(args, ["stash", "apply", "--index", "stash@{0}"]);
        repo.run(&args);
        assert!(
            !repo
                .git(&["diff", "--cached", "--", "tracked.txt"])
                .is_empty()
        );
    }

    #[test]
    fn selected_stash_oid_prevents_actions_on_a_shifted_index() {
        let repo = TestRepo::new();
        fs::write(repo.0.join("tracked.txt"), "first stash\n").unwrap();
        repo.run(&build_push_args(&opts("first"), true).unwrap());
        let git_repo = Repository::open(&repo.0).unwrap();
        let pinned = SelectedStash {
            repo_path: repo.0.clone(),
            index: 0,
            oid: stash_oid(&git_repo, 0).unwrap(),
        };
        fs::write(repo.0.join("tracked.txt"), "second stash\n").unwrap();
        repo.run(&build_push_args(&opts("second"), true).unwrap());
        assert!(validated_stash_action(&git_repo, &repo.0, &pinned, "apply", false).is_err());
        assert!(validated_drop_action(&git_repo, &repo.0, &pinned).is_err());
    }
}
