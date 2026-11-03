use super::*;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum ResetMode {
    Soft,
    #[default]
    Mixed,
    Hard,
}

impl ResetMode {
    fn git_arg(self) -> &'static str {
        match self {
            Self::Soft => "--soft",
            Self::Mixed => "--mixed",
            Self::Hard => "--hard",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Soft => "Soft",
            Self::Mixed => "Mixed",
            Self::Hard => "Hard",
        }
    }

    fn effects(self) -> &'static str {
        match self {
            Self::Soft => "Moves HEAD only. The index and working tree stay unchanged.",
            Self::Mixed => "Moves HEAD and resets the index. Working tree files stay unchanged.",
            Self::Hard => {
                "Moves HEAD, resets the index, and replaces tracked working tree files. Uncommitted tracked changes are lost; untracked files that obstruct checkout may also be removed."
            }
        }
    }
}

#[derive(Clone)]
struct PendingReset {
    repo_path: PathBuf,
    head: Option<(String, String)>,
    selected_commit: String,
    target_oid: String,
    mode: ResetMode,
    hard_acknowledged: bool,
}

#[derive(Default)]
pub(crate) struct ResetTools {
    mode: ResetMode,
    pending: Option<PendingReset>,
    error: Option<String>,
}

fn resolve_oid(repo: &Repository, revision: &str) -> Result<String, String> {
    repo.revparse_single(revision)
        .and_then(|object| object.peel_to_commit())
        .map(|commit| commit.id().to_string())
        .map_err(|error| format!("Could not resolve selected commit: {error}"))
}

fn head_state(repo: &Repository) -> Option<(String, String)> {
    repo.head().ok().map(|head| {
        let name = head.name().unwrap_or("HEAD").to_owned();
        let oid = head.target().map(|id| id.to_string()).unwrap_or_default();
        (name, oid)
    })
}

fn repository_state_is_clean(repo: &Repository) -> bool {
    repo.state() == git2::RepositoryState::Clean
}

fn confirmation_is_valid(
    pending: &PendingReset,
    repo_path: &Path,
    selected_commit: &str,
    mode: ResetMode,
    repo: &Repository,
) -> bool {
    pending.repo_path == repo_path
        && pending.selected_commit == selected_commit
        && pending.mode == mode
        && pending.head == head_state(repo)
        && repository_state_is_clean(repo)
}

impl GritApp {
    pub(crate) fn reset_tools_ui(&mut self, ui: &mut egui::Ui) {
        ui.group(|ui| {
            ui.heading("Reset current branch");
            let busy = self.operation.is_some();
            let Some(repo) = self.repo() else {
                ui.label("Open a repository first.");
                self.reset_tools.pending = None;
                return;
            };
            let Some(selected) = self.selected_commit.clone() else {
                ui.label("Select a commit in the history first.");
                self.reset_tools.pending = None;
                return;
            };
            if !repository_state_is_clean(&repo) {
                    self.reset_tools.pending = None;
                    ui.colored_label(Color32::YELLOW, "Reset is unavailable while a merge, rebase, cherry-pick, or other Git operation is in progress. Continue or abort that operation first.");
                    return;
            }
            ui.label(format!("Selected commit: {}", &selected[..selected.len().min(12)]));
            egui::ComboBox::from_id_salt("reset_mode")
                .selected_text(self.reset_tools.mode.label())
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.reset_tools.mode, ResetMode::Soft, "Soft");
                    ui.selectable_value(&mut self.reset_tools.mode, ResetMode::Mixed, "Mixed");
                    ui.selectable_value(&mut self.reset_tools.mode, ResetMode::Hard, "Hard");
                });
            ui.label(self.reset_tools.mode.effects());
            ui.add_enabled_ui(!busy, |ui| {
                if ui.button("Review reset…").clicked() {
                    self.reset_tools.error = None;
                    match resolve_oid(&repo, &selected) {
                        Ok(target_oid) => {
                            self.reset_tools.pending = Some(PendingReset {
                                repo_path: self.path.clone().unwrap_or_default(),
                                head: head_state(&repo),
                                selected_commit: selected.clone(),
                                target_oid,
                                mode: self.reset_tools.mode,
                                hard_acknowledged: false,
                            });
                        }
                        Err(error) => self.reset_tools.error = Some(error),
                    }
                }
            });
            if let Some(error) = &self.reset_tools.error {
                ui.colored_label(Color32::RED, error);
            }
            let valid = self.reset_tools.pending.as_ref().is_some_and(|pending| {
                self.path.as_deref().is_some_and(|path| {
                    confirmation_is_valid(
                        pending,
                        path,
                        &selected,
                        self.reset_tools.mode,
                        &repo,
                    )
                })
            });
            if self.reset_tools.pending.is_some() && !valid {
                self.reset_tools.pending = None;
                ui.colored_label(Color32::YELLOW, "Reset review expired because the repository, HEAD, selection, or mode changed. Review again.");
            }
            let mut confirm_args = None;
            let mut cancel = false;
            if let Some(pending) = self.reset_tools.pending.as_mut() {
                ui.separator();
                ui.label(format!("{} reset to {}", pending.mode.label(), pending.target_oid));
                ui.label(pending.mode.effects());
                if pending.mode == ResetMode::Hard {
                    ui.checkbox(&mut pending.hard_acknowledged, "I understand tracked uncommitted changes will be lost");
                }
                ui.horizontal(|ui| {
                    let can_reset = !busy && valid && (pending.mode != ResetMode::Hard || pending.hard_acknowledged);
                    if ui.add_enabled(can_reset, egui::Button::new("Confirm reset")).clicked() {
                        confirm_args = Some(vec!["reset".to_owned(), pending.mode.git_arg().to_owned(), pending.target_oid.clone()]);
                    }
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                });
            }
            if let Some(args) = confirm_args {
                self.start_git_command("Reset", args);
                self.reset_tools.pending = None;
            } else if cancel {
                self.reset_tools.pending = None;
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        process::Command,
        time::{SystemTime, UNIX_EPOCH},
    };

    struct TestRepo(PathBuf);
    impl TestRepo {
        fn new() -> Self {
            let id = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!("grit-reset-{id}"));
            fs::create_dir_all(&path).unwrap();
            let repo = Self(path);
            repo.git(&["init", "-q"]);
            repo.git(&["config", "user.name", "Reset Test"]);
            repo.git(&["config", "user.email", "reset@example.test"]);
            fs::write(repo.0.join("tracked.txt"), "first\n").unwrap();
            repo.git(&["add", "tracked.txt"]);
            repo.git(&["commit", "-qm", "first"]);
            fs::write(repo.0.join("tracked.txt"), "second\n").unwrap();
            repo.git(&["commit", "-qam", "second"]);
            repo
        }
        fn git(&self, args: &[&str]) -> String {
            let out = Command::new("git")
                .arg("-C")
                .arg(&self.0)
                .args(args)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            String::from_utf8_lossy(&out.stdout).trim().to_owned()
        }
    }
    impl Drop for TestRepo {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn reset_modes_have_expected_head_index_and_worktree_effects() {
        for (mode, index_expected, worktree_expected) in [
            (ResetMode::Soft, "dirty\n", "dirty\n"),
            (ResetMode::Mixed, "first\n", "dirty\n"),
            (ResetMode::Hard, "first\n", "first\n"),
        ] {
            let repo = TestRepo::new();
            let first = repo.git(&["rev-parse", "HEAD~1"]);
            fs::write(repo.0.join("tracked.txt"), "dirty\n").unwrap();
            if mode == ResetMode::Soft {
                repo.git(&["add", "tracked.txt"]);
            }
            let reset = Command::new("git")
                .arg("-C")
                .arg(&repo.0)
                .args(["reset", mode.git_arg(), &first])
                .output()
                .unwrap();
            assert!(reset.status.success());
            assert_eq!(repo.git(&["rev-parse", "HEAD"]), first);
            assert_eq!(repo.git(&["show", ":tracked.txt"]), index_expected.trim());
            assert_eq!(
                fs::read_to_string(repo.0.join("tracked.txt")).unwrap(),
                worktree_expected
            );
        }
    }

    #[test]
    fn confirmation_target_is_a_full_resolved_object_id_and_modes_are_explicit() {
        let repo = TestRepo::new();
        let target = resolve_oid(&Repository::open(&repo.0).unwrap(), "HEAD~1").unwrap();
        assert_eq!(target, repo.git(&["rev-parse", "HEAD~1"]));
        assert_eq!(ResetMode::Soft.git_arg(), "--soft");
        assert_eq!(ResetMode::Mixed.git_arg(), "--mixed");
        assert_eq!(ResetMode::Hard.git_arg(), "--hard");
        assert!(ResetMode::Hard.effects().contains("lost"));
        assert!(ResetMode::Hard.effects().contains("untracked files"));
    }

    #[test]
    fn confirmation_expires_when_symbolic_head_changes_even_at_same_oid() {
        let repo = TestRepo::new();
        let git_repo = Repository::open(&repo.0).unwrap();
        let selected = repo.git(&["rev-parse", "HEAD"]);
        let pending = PendingReset {
            repo_path: repo.0.clone(),
            head: head_state(&git_repo),
            selected_commit: selected.clone(),
            target_oid: repo.git(&["rev-parse", "HEAD~1"]),
            mode: ResetMode::Mixed,
            hard_acknowledged: false,
        };
        assert!(confirmation_is_valid(
            &pending,
            &repo.0,
            &selected,
            ResetMode::Mixed,
            &git_repo
        ));
        repo.git(&["switch", "-c", "same-commit"]);
        assert_eq!(
            head_state(&git_repo).unwrap().1,
            pending.head.as_ref().unwrap().1
        );
        assert!(!confirmation_is_valid(
            &pending,
            &repo.0,
            &selected,
            ResetMode::Mixed,
            &git_repo
        ));
    }

    #[test]
    fn confirmation_expires_when_a_git_operation_starts() {
        let repo = TestRepo::new();
        let git_repo = Repository::open(&repo.0).unwrap();
        assert!(repository_state_is_clean(&git_repo));
        let selected = repo.git(&["rev-parse", "HEAD"]);
        let pending = PendingReset {
            repo_path: repo.0.clone(),
            head: head_state(&git_repo),
            selected_commit: selected.clone(),
            target_oid: repo.git(&["rev-parse", "HEAD~1"]),
            mode: ResetMode::Mixed,
            hard_acknowledged: false,
        };
        fs::write(repo.0.join("tracked.txt"), "staged changes are allowed\n").unwrap();
        repo.git(&["add", "tracked.txt"]);
        assert!(repository_state_is_clean(&git_repo));
        assert!(confirmation_is_valid(
            &pending,
            &repo.0,
            &selected,
            ResetMode::Mixed,
            &git_repo
        ));
        fs::write(repo.0.join(".git/MERGE_HEAD"), format!("{selected}\n")).unwrap();
        assert!(!repository_state_is_clean(&git_repo));
        assert!(!confirmation_is_valid(
            &pending,
            &repo.0,
            &selected,
            ResetMode::Mixed,
            &git_repo
        ));
    }
}
