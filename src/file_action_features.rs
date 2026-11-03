use super::*;

#[derive(Clone, Debug, PartialEq, Eq)]
struct IndexEntryState {
    oid: git2::Oid,
    mode: u32,
    flags: u16,
    flags_extended: u16,
}

#[derive(Clone)]
struct PendingDiscard {
    repo_path: PathBuf,
    path: String,
    raw_diff: String,
    display_diff: String,
    selected_lines: HashSet<usize>,
    reverse_patch: String,
    index_state: Vec<IndexEntryState>,
    head: Option<(String, String)>,
}

#[derive(Default)]
pub(crate) struct FileActions {
    pending_discard: Option<PendingDiscard>,
    message: Option<String>,
}

fn valid_repo_path(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && !path.contains('\0')
        && path
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
}

fn index_state(repo: &Repository, path: &str) -> Result<Vec<IndexEntryState>, String> {
    let mut index = repo.index().map_err(|error| error.message().to_owned())?;
    index
        .read(true)
        .map_err(|error| error.message().to_owned())?;
    let mut entries = index
        .iter()
        .filter(|entry| entry.path == path.as_bytes())
        .map(|entry| IndexEntryState {
            oid: entry.id,
            mode: entry.mode,
            flags: entry.flags,
            flags_extended: entry.flags_extended,
        })
        .collect::<Vec<_>>();
    entries.sort_by_key(|entry| (entry.mode, entry.oid, entry.flags, entry.flags_extended));
    Ok(entries)
}

fn head_state(repo: &Repository) -> Option<(String, String)> {
    repo.head().ok().map(|head| {
        (
            head.name().unwrap_or("HEAD").to_owned(),
            head.target().map(|oid| oid.to_string()).unwrap_or_default(),
        )
    })
}

fn prepare_discard(
    repo_path: &Path,
    repo: &Repository,
    path: &str,
    raw_diff: &str,
    display_diff: &str,
    selected_lines: &HashSet<usize>,
) -> Result<PendingDiscard, String> {
    if !valid_repo_path(path) {
        return Err("Select a repository-relative file path.".into());
    }
    let selectable = diff_features::selectable_lines(display_diff);
    if selected_lines.is_empty() || !selected_lines.iter().all(|line| selectable.contains(line)) {
        return Err("Select one or more added or deleted lines first.".into());
    }
    let fresh_diff = diff_file_side(repo, path, false);
    if fresh_diff != raw_diff {
        return Err(
            "The file changed since the diff was shown. Refresh and select lines again.".into(),
        );
    }
    let reverse_patch = diff_features::selected_patch(display_diff, selected_lines, true)?;
    if reverse_patch.is_empty() {
        return Err("The selected patch contains no changes.".into());
    }
    Ok(PendingDiscard {
        repo_path: repo_path.to_path_buf(),
        path: path.to_owned(),
        raw_diff: raw_diff.to_owned(),
        display_diff: display_diff.to_owned(),
        selected_lines: selected_lines.clone(),
        reverse_patch,
        index_state: index_state(repo, path)?,
        head: head_state(repo),
    })
}

fn validate_discard_snapshot(
    pending: &PendingDiscard,
    repo_path: &Path,
    repo: &Repository,
) -> Result<(), String> {
    if pending.repo_path != repo_path {
        return Err("The repository changed after the discard preview.".into());
    }
    if head_state(repo) != pending.head {
        return Err("HEAD changed after the discard preview. Review the patch again.".into());
    }
    if index_state(repo, &pending.path)? != pending.index_state {
        return Err("The index changed after the discard preview. Review the patch again.".into());
    }
    if diff_file_side(repo, &pending.path, false) != pending.raw_diff {
        return Err("The file changed after the discard preview. Review the patch again.".into());
    }
    Ok(())
}

fn apply_discard_patch(repo_path: &Path, pending: &PendingDiscard) -> Result<(), String> {
    if !valid_repo_path(&pending.path) {
        return Err("The selected path is not repository-relative.".into());
    }
    let repo = Repository::open(repo_path).map_err(|error| error.message().to_owned())?;
    validate_discard_snapshot(pending, repo_path, &repo)?;
    let mut child = Command::new("git")
        .arg("-C")
        .arg(repo_path)
        .args(["apply", "--recount", "--unidiff-zero", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| error.to_string())?;
    child
        .stdin
        .take()
        .ok_or_else(|| "Could not open Git input.".to_owned())?
        .write_all(pending.reverse_patch.as_bytes())
        .map_err(|error| error.to_string())?;
    let output = child
        .wait_with_output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }
    let repo = Repository::open(repo_path).map_err(|error| error.message().to_owned())?;
    let after = index_state(&repo, &pending.path)?;
    if after != pending.index_state {
        return Err(
            "Git changed the index while discarding lines; inspect the repository state.".into(),
        );
    }
    Ok(())
}

fn executable_mode(mode: u32) -> Option<bool> {
    match mode {
        0o100644 => Some(false),
        0o100755 => Some(true),
        _ => None,
    }
}

fn set_index_executable(
    repo_path: &Path,
    path: &str,
    expected: &IndexEntryState,
    make_executable: bool,
) -> Result<(), String> {
    if !valid_repo_path(path) || !regular_worktree_file(repo_path, path) {
        return Err("Select a tracked regular file in the worktree.".into());
    }
    let repo = Repository::open(repo_path).map_err(|error| error.message().to_owned())?;
    let mut index = repo.index().map_err(|error| error.message().to_owned())?;
    index
        .read(true)
        .map_err(|error| error.message().to_owned())?;
    let mut entries = index
        .iter()
        .filter(|entry| entry.path == path.as_bytes())
        .collect::<Vec<_>>();
    if entries.len() != 1 {
        return Err("The index entry changed or has conflicts. Refresh the file list.".into());
    }
    let entry = entries.remove(0);
    let actual = IndexEntryState {
        oid: entry.id,
        mode: entry.mode,
        flags: entry.flags,
        flags_extended: entry.flags_extended,
    };
    if actual != *expected || entry.flags & 0x3000 != 0 {
        return Err("The index entry changed. Refresh the file list and try again.".into());
    }
    if executable_mode(entry.mode).is_none() {
        return Err("Executable bit is not available for symlinks or submodules.".into());
    }
    let desired_mode = if make_executable { 0o100755 } else { 0o100644 };
    if entry.mode == desired_mode {
        return Ok(());
    }
    let replacement = git2::IndexEntry {
        ctime: entry.ctime,
        mtime: entry.mtime,
        dev: entry.dev,
        ino: entry.ino,
        mode: desired_mode,
        uid: entry.uid,
        gid: entry.gid,
        file_size: entry.file_size,
        id: entry.id,
        flags: entry.flags,
        flags_extended: entry.flags_extended,
        path: entry.path,
    };
    index
        .add(&replacement)
        .map_err(|error| error.message().to_owned())?;
    index.write().map_err(|error| error.message().to_owned())?;

    let after = index_state(&repo, path)?;
    if after.len() != 1 || after[0].oid != expected.oid || after[0].mode != desired_mode {
        return Err("The index changed unexpectedly while updating its executable mode.".into());
    }
    Ok(())
}

fn regular_worktree_file(repo_path: &Path, path: &str) -> bool {
    valid_repo_path(path)
        && std::fs::symlink_metadata(repo_path.join(path))
            .is_ok_and(|metadata| metadata.file_type().is_file())
}

impl GritApp {
    pub(crate) fn selected_discard_ui(&mut self, ui: &mut egui::Ui) {
        let eligible = self.tab == 0
            && !self.selected_staged
            && !self.diff_preferences.ignore_whitespace
            && !self.selected_diff_lines.is_empty()
            && self.selected_file.is_some()
            && self.operation.is_none();
        ui.add_enabled_ui(eligible, |ui| {
            if ui.button("Discard selected unstaged lines… (R)").clicked() {
                self.open_discard_review();
            }
        });
        if eligible
            && !ui.ctx().wants_keyboard_input()
            && ui
                .ctx()
                .input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::R))
        {
            self.open_discard_review();
        }
        if let Some(message) = self.file_actions.message.take() {
            ui.colored_label(Color32::YELLOW, message);
        }
        self.discard_confirmation_ui(ui);
    }

    fn open_discard_review(&mut self) {
        if self.operation.is_some()
            || self.tab != 0
            || self.selected_staged
            || self.diff_preferences.ignore_whitespace
        {
            return;
        }
        let (Some(repo_path), Some(repo), Some(path)) =
            (self.path.clone(), self.repo(), self.selected_file.clone())
        else {
            return;
        };
        self.ensure_display_diff();
        match prepare_discard(
            &repo_path,
            &repo,
            &path,
            &self.diff,
            &self.display_diff,
            &self.selected_diff_lines,
        ) {
            Ok(pending) => self.file_actions.pending_discard = Some(pending),
            Err(error) => self.file_actions.message = Some(error),
        }
    }

    fn discard_confirmation_ui(&mut self, ui: &mut egui::Ui) {
        let Some(pending) = self.file_actions.pending_discard.clone() else {
            return;
        };
        egui::Window::new("Discard selected lines?")
            .collapsible(false)
            .resizable(true)
            .default_width(620.)
            .show(ui.ctx(), |ui| {
                ui.label(format!("File: {}", pending.path));
                ui.label("Only the selected unstaged changes will be reversed in the working tree. The index and other edits remain unchanged.");
                ui.label("Patch preview:");
                egui::ScrollArea::vertical().max_height(260.).show(ui, |ui| {
                    ui.label(RichText::new(&pending.reverse_patch).monospace());
                });
                ui.horizontal(|ui| {
                    if ui.add_enabled(self.operation.is_none(), egui::Button::new("Confirm discard")).clicked() {
                        self.confirm_discard(pending.clone());
                    }
                    if ui.button("Cancel").clicked() {
                        self.file_actions.pending_discard = None;
                    }
                });
            });
    }

    fn confirm_discard(&mut self, pending: PendingDiscard) {
        let current_selection_matches = self.path.as_ref() == Some(&pending.repo_path)
            && self.selected_file.as_deref() == Some(&pending.path)
            && self.tab == 0
            && !self.selected_staged
            && !self.diff_preferences.ignore_whitespace
            && self.diff == pending.raw_diff
            && self.display_diff == pending.display_diff
            && self.selected_diff_lines == pending.selected_lines;
        let validation = self
            .repo()
            .ok_or_else(|| "Open the same repository to continue.".to_owned())
            .and_then(|repo| validate_discard_snapshot(&pending, &pending.repo_path, &repo));
        if !current_selection_matches || validation.is_err() {
            self.file_actions.pending_discard = None;
            self.file_actions.message = Some(validation.err().unwrap_or_else(|| {
                "The file, selection, mode, or repository changed. Review the discard again.".into()
            }));
            return;
        }
        let root = pending.repo_path.clone();
        let worker_pending = pending.clone();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let result = apply_discard_patch(&root, &worker_pending)
                .map(|_| "Selected unstaged lines discarded; index unchanged.".to_owned());
            let _ = tx.send(result);
        });
        self.notice = "Discard selected lines in progress…".into();
        self.operation = Some(("Discard selected lines".into(), rx));
        self.file_actions.pending_discard = None;
        self.selected_diff_lines.clear();
    }

    pub(crate) fn executable_bit_ui(&mut self, ui: &mut egui::Ui) {
        let Some(path) = self.selected_file.clone() else {
            return;
        };
        let Some(root) = self.path.clone() else {
            return;
        };
        let Some(repo) = self.repo() else {
            return;
        };
        let Ok(mut index) = repo.index() else {
            ui.colored_label(Color32::YELLOW, "Could not read the Git index.");
            return;
        };
        if index.read(true).is_err() {
            ui.colored_label(Color32::YELLOW, "Could not refresh the Git index.");
            return;
        }
        let entries = index
            .iter()
            .filter(|entry| entry.path == path.as_bytes())
            .collect::<Vec<_>>();
        let Some(entry) = entries.iter().find(|entry| entry.flags & 0x3000 == 0) else {
            ui.label("Executable bit is available for tracked regular files only.");
            return;
        };
        let Some(is_executable) = executable_mode(entry.mode) else {
            ui.label("Executable bit is not available for symlinks or submodules.");
            return;
        };
        if entries.len() != 1 || !regular_worktree_file(&root, &path) {
            ui.label("Executable bit requires one tracked regular file; conflicts and symlinks are unsupported.");
            return;
        }
        ui.label(if is_executable {
            "The index records this file as executable."
        } else {
            "The index records this file as non-executable."
        });
        ui.label("This changes only the index mode. Working tree bytes and filesystem permissions stay unchanged.");
        let expected = IndexEntryState {
            oid: entry.id,
            mode: entry.mode,
            flags: entry.flags,
            flags_extended: entry.flags_extended,
        };
        if ui
            .add_enabled(
                self.operation.is_none(),
                egui::Button::new("Stage executable bit in index"),
            )
            .clicked()
        {
            let root = root.clone();
            let make_executable = !is_executable;
            let (tx, rx) = mpsc::channel();
            std::thread::spawn(move || {
                let result =
                    set_index_executable(&root, &path, &expected, make_executable).map(|_| {
                        "Executable mode staged in the index; blob and worktree unchanged."
                            .to_owned()
                    });
                let _ = tx.send(result);
            });
            self.notice = "Updating executable mode in the index…".into();
            self.operation = Some(("Stage executable bit in index".into(), rx));
        }
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
            Self::with_file("tracked.txt", "base\n")
        }

        fn with_file(name: &str, contents: &str) -> Self {
            let stamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!("grit-file-actions-{stamp}"));
            fs::create_dir_all(&path).unwrap();
            let repo = Self(path);
            repo.git(&["init", "-q"]);
            repo.git(&["config", "user.name", "File Action Test"]);
            repo.git(&["config", "user.email", "file-action@example.test"]);
            repo.git(&["config", "core.filemode", "true"]);
            fs::write(repo.0.join(name), contents).unwrap();
            let literal_path = format!(":(literal){name}");
            repo.git(&["add", "--", &literal_path]);
            repo.git(&["commit", "-qm", "initial"]);
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

    fn diff_with_context(repo: &Repository, path: &str, context: u32) -> String {
        let mut options = git2::DiffOptions::new();
        options
            .pathspec(path)
            .disable_pathspec_match(true)
            .include_untracked(true)
            .recurse_untracked_dirs(true)
            .show_untracked_content(true)
            .context_lines(context);
        let diff = repo
            .diff_index_to_workdir(None, Some(&mut options))
            .unwrap();
        let mut patch = String::new();
        diff.print(git2::DiffFormat::Patch, |_, _, line| {
            if matches!(line.origin(), '+' | '-' | ' ') {
                patch.push(line.origin());
            }
            patch.push_str(std::str::from_utf8(line.content()).unwrap_or(""));
            true
        })
        .unwrap();
        patch
    }

    fn choose_lines(patch: &str, wanted: &[&str]) -> HashSet<usize> {
        let selectable = diff_features::selectable_lines(patch);
        let lines: Vec<_> = patch.lines().collect();
        wanted
            .iter()
            .map(|wanted| {
                selectable
                    .iter()
                    .find(|&&index| lines.get(index).is_some_and(|line| line == wanted))
                    .copied()
                    .unwrap_or_else(|| panic!("patch line not found: {wanted}\n{patch}"))
            })
            .collect()
    }

    fn staged_file_state(repo: &Repository, path: &str) -> (git2::Oid, u32) {
        let mut index = repo.index().unwrap();
        index.read(true).unwrap();
        let entry = index.get_path(Path::new(path), 0).unwrap();
        (entry.id, entry.mode)
    }

    fn all_index_states(repo: &Repository) -> Vec<(Vec<u8>, git2::Oid, u32)> {
        let mut index = repo.index().unwrap();
        index.read(true).unwrap();
        let mut entries = index
            .iter()
            .map(|entry| (entry.path, entry.id, entry.mode))
            .collect::<Vec<_>>();
        entries.sort();
        entries
    }

    #[test]
    fn reverse_selected_mixed_edits_preserve_index_and_other_worktree_edits_context_zero_and_three()
    {
        for context in [0, 3] {
            let lines = (1..=24).map(|n| format!("line{n}")).collect::<Vec<_>>();
            let repo = TestRepo::with_file("mixed.txt", &format!("{}\n", lines.join("\n")));
            let repository = Repository::open(&repo.0).unwrap();
            let mut staged = lines.clone();
            staged[1] = "stagedTwo".into();
            fs::write(repo.0.join("mixed.txt"), format!("{}\n", staged.join("\n"))).unwrap();
            repo.git(&["add", "--", "mixed.txt"]);
            let before_discard_index = staged_file_state(&repository, "mixed.txt");

            let mut worktree = staged.clone();
            worktree[3] = "unstagedFour".into();
            worktree.remove(17);
            worktree.push("unstagedEnd".into());
            fs::write(
                repo.0.join("mixed.txt"),
                format!("{}\n", worktree.join("\n")),
            )
            .unwrap();
            let raw = diff_file_side(&repository, "mixed.txt", false);
            let display = diff_with_context(&repository, "mixed.txt", context);
            let selected = choose_lines(&display, &["+unstagedFour", "-line4", "-line18"]);
            let pending =
                prepare_discard(&repo.0, &repository, "mixed.txt", &raw, &display, &selected)
                    .unwrap();
            assert!(pending.reverse_patch.contains("-unstagedFour"));
            assert!(pending.reverse_patch.contains("+line18"));
            apply_discard_patch(&repo.0, &pending).unwrap();

            let mut expected = staged;
            expected[3] = "line4".into();
            expected.push("unstagedEnd".into());
            assert_eq!(
                fs::read_to_string(repo.0.join("mixed.txt")).unwrap(),
                format!("{}\n", expected.join("\n")),
                "context {context}; reverse patch:\n{}",
                pending.reverse_patch
            );
            assert_eq!(
                staged_file_state(&repository, "mixed.txt"),
                before_discard_index
            );
        }
    }

    #[test]
    fn reverse_selected_patch_preserves_crlf_and_handles_new_and_deleted_files() {
        let repo = TestRepo::with_file("crlf.txt", "one\r\ntwo\r\nthree\r\nfour\r\n");
        let repository = Repository::open(&repo.0).unwrap();
        let original = staged_file_state(&repository, "crlf.txt");
        fs::write(
            repo.0.join("crlf.txt"),
            "one\r\ntwo changed\r\nthree\r\nfour\r\n",
        )
        .unwrap();
        let raw = diff_file_side(&repository, "crlf.txt", false);
        let display = diff_with_context(&repository, "crlf.txt", 0);
        let selected = choose_lines(&display, &["+two changed", "-two"]);
        let pending =
            prepare_discard(&repo.0, &repository, "crlf.txt", &raw, &display, &selected).unwrap();
        apply_discard_patch(&repo.0, &pending).unwrap();
        assert_eq!(
            fs::read(repo.0.join("crlf.txt")).unwrap(),
            b"one\r\ntwo\r\nthree\r\nfour\r\n"
        );
        assert_eq!(staged_file_state(&repository, "crlf.txt"), original);

        fs::write(repo.0.join("new.txt"), "first\nkeep\n").unwrap();
        let raw = diff_file_side(&repository, "new.txt", false);
        let display = diff_with_context(&repository, "new.txt", 3);
        let selected = choose_lines(&display, &["+first"]);
        let pending =
            prepare_discard(&repo.0, &repository, "new.txt", &raw, &display, &selected).unwrap();
        apply_discard_patch(&repo.0, &pending).unwrap();
        assert_eq!(
            fs::read_to_string(repo.0.join("new.txt")).unwrap(),
            "keep\n"
        );

        let deleted = TestRepo::with_file("deleted.txt", "restore me\nleave deleted\n");
        let deleted_repo = Repository::open(&deleted.0).unwrap();
        let index_before = staged_file_state(&deleted_repo, "deleted.txt");
        fs::remove_file(deleted.0.join("deleted.txt")).unwrap();
        let raw = diff_file_side(&deleted_repo, "deleted.txt", false);
        let display = diff_with_context(&deleted_repo, "deleted.txt", 3);
        let selected = choose_lines(&display, &["-restore me"]);
        let pending = prepare_discard(
            &deleted.0,
            &deleted_repo,
            "deleted.txt",
            &raw,
            &display,
            &selected,
        )
        .unwrap();
        apply_discard_patch(&deleted.0, &pending).unwrap();
        assert_eq!(
            fs::read_to_string(deleted.0.join("deleted.txt")).unwrap(),
            "restore me\n"
        );
        assert_eq!(
            staged_file_state(&deleted_repo, "deleted.txt"),
            index_before
        );
    }

    #[test]
    fn discard_confirmation_rejects_stale_diff_index_and_repository() {
        let repo = TestRepo::new();
        let repository = Repository::open(&repo.0).unwrap();
        fs::write(repo.0.join("tracked.txt"), "changed\n").unwrap();
        let raw = diff_file_side(&repository, "tracked.txt", false);
        let display = diff_with_context(&repository, "tracked.txt", 3);
        let selected = choose_lines(&display, &["+changed"]);
        let pending = prepare_discard(
            &repo.0,
            &repository,
            "tracked.txt",
            &raw,
            &display,
            &selected,
        )
        .unwrap();
        validate_discard_snapshot(&pending, &repo.0, &repository).unwrap();
        assert!(
            validate_discard_snapshot(&pending, Path::new("/other/repo"), &repository).is_err()
        );
        fs::write(repo.0.join("tracked.txt"), "changed again\n").unwrap();
        assert!(validate_discard_snapshot(&pending, &repo.0, &repository).is_err());

        fs::write(repo.0.join("tracked.txt"), "changed\n").unwrap();
        let raw = diff_file_side(&repository, "tracked.txt", false);
        let display = diff_with_context(&repository, "tracked.txt", 3);
        let selected = choose_lines(&display, &["+changed"]);
        let pending = prepare_discard(
            &repo.0,
            &repository,
            "tracked.txt",
            &raw,
            &display,
            &selected,
        )
        .unwrap();
        fs::write(repo.0.join("tracked.txt"), "staged\n").unwrap();
        repo.git(&["add", "--", "tracked.txt"]);
        assert!(validate_discard_snapshot(&pending, &repo.0, &repository).is_err());
    }

    #[test]
    fn executable_index_toggle_preserves_blob_and_worktree_and_uses_literal_path() {
        let name = "literal [*].sh";
        let repo = TestRepo::with_file(name, "#!/bin/sh\necho safe\n");
        fs::write(repo.0.join(name), "#!/bin/sh\necho staged\n").unwrap();
        let literal = format!(":(literal){name}");
        repo.git(&["add", "--", &literal]);
        fs::write(repo.0.join(name), "#!/bin/sh\necho unstaged\n").unwrap();
        fs::write(repo.0.join("sibling.sh"), "#!/bin/sh\necho sibling\n").unwrap();
        repo.git(&["add", "--", "sibling.sh"]);
        let repository = Repository::open(&repo.0).unwrap();
        let before = staged_file_state(&repository, name);
        let index_before = all_index_states(&repository);
        let bytes = fs::read(repo.0.join(name)).unwrap();
        let permissions = fs::metadata(repo.0.join(name)).unwrap().permissions();
        let expected = index_state(&repository, name).unwrap().remove(0);
        set_index_executable(&repo.0, name, &expected, true).unwrap();
        let executable = staged_file_state(&repository, name);
        assert_eq!(executable.0, before.0);
        assert_eq!(executable.1, 0o100755);
        assert_eq!(all_index_states(&repository).len(), index_before.len());
        assert_eq!(
            all_index_states(&repository)
                .into_iter()
                .find(|(path, _, _)| path == name.as_bytes())
                .map(|(_, oid, mode)| (oid, mode)),
            Some((before.0, 0o100755))
        );
        assert_eq!(
            all_index_states(&repository)
                .into_iter()
                .find(|(path, _, _)| path == b"sibling.sh"),
            index_before
                .iter()
                .find(|(path, _, _)| path == b"sibling.sh")
                .cloned()
        );
        assert_eq!(fs::read(repo.0.join(name)).unwrap(), bytes);
        assert_eq!(
            fs::metadata(repo.0.join(name)).unwrap().permissions(),
            permissions
        );

        let expected = index_state(&repository, name).unwrap().remove(0);
        set_index_executable(&repo.0, name, &expected, false).unwrap();
        assert_eq!(staged_file_state(&repository, name), before);
        assert!(executable_mode(0o120000).is_none());
        assert!(executable_mode(0o160000).is_none());
        assert!(!regular_worktree_file(&repo.0, "missing"));
    }
}
