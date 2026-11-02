use super::*;
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::Path,
};

#[derive(Default)]
pub(crate) struct RepositoryTools {
    rename_from: String,
    rename_to: String,
    merge_branch: String,
    tag_name: String,
    tag_message: String,
    tag_target: String,
    pending_tag_delete: Option<String>,
    ignore_path: String,
    ignore_raw_pattern: bool,
}

/// Convert a repository relative path into a root-anchored literal gitignore pattern.
fn literal_ignore_pattern(path: &str) -> Result<String, String> {
    if path.is_empty()
        || path.starts_with('/')
        || path.contains('\0')
        || path.contains('\n')
        || path.contains('\r')
    {
        return Err("Enter a repository-relative path without line breaks".into());
    }
    #[cfg(windows)]
    let normalized = path.replace('\\', "/");
    #[cfg(not(windows))]
    let normalized = path.to_owned();
    let parts: Vec<_> = normalized.split('/').collect();
    if parts
        .iter()
        .any(|p| p.is_empty() || *p == "." || *p == "..")
    {
        return Err(
            "Path must stay inside the repository and cannot contain empty or dot components"
                .into(),
        );
    }
    let mut out = String::from("/");
    for (i, part) in parts.iter().enumerate() {
        if i > 0 {
            out.push('/');
        }
        for (j, c) in part.chars().enumerate() {
            if (j == 0 && matches!(c, '#' | '!'))
                || matches!(c, '\\' | '*' | '?' | '[' | ']')
                || (c == ' ' && j + 1 == part.chars().count())
            {
                out.push('\\');
            }
            out.push(c);
        }
    }
    Ok(out)
}

fn append_ignore(root: &Path, path: &str) -> Result<String, String> {
    let pattern = literal_ignore_pattern(path)?;
    append_pattern(root, &pattern)
}

fn append_raw_ignore(root: &Path, pattern: &str) -> Result<String, String> {
    if pattern.is_empty()
        || pattern.contains('\n')
        || pattern.contains('\r')
        || pattern.contains('\0')
    {
        return Err("Enter a non-empty pattern without line breaks".into());
    }
    append_pattern(root, pattern)
}

fn append_pattern(root: &Path, pattern: &str) -> Result<String, String> {
    let file = root.join(".gitignore");
    if let Ok(meta) = fs::symlink_metadata(&file) {
        if meta.file_type().is_symlink() {
            return Err("Refusing to write through a .gitignore symlink".into());
        }
        if !meta.is_file() {
            return Err("Repository .gitignore is not a regular file".into());
        }
    }
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        #[cfg(target_os = "linux")]
        options.custom_flags(0x20000); // Linux O_NOFOLLOW
        #[cfg(any(
            target_os = "macos",
            target_os = "ios",
            target_os = "freebsd",
            target_os = "openbsd",
            target_os = "netbsd",
            target_os = "dragonfly"
        ))]
        options.custom_flags(0x100); // BSD-family O_NOFOLLOW
    }
    let mut file = options
        .open(&file)
        .map_err(|e| format!("Could not open repository .gitignore: {e}"))?;
    let len = file.metadata().map_err(|e| e.to_string())?.len();
    if len > 0 {
        // Ensure the new pattern starts on its own line without changing existing bytes.
        let existing = fs::read(root.join(".gitignore")).map_err(|e| e.to_string())?;
        if !existing.ends_with(b"\n") {
            file.write_all(b"\n").map_err(|e| e.to_string())?;
        }
    }
    writeln!(file, "{pattern}")
        .map_err(|e| format!("Could not update repository .gitignore: {e}"))?;
    Ok(pattern.to_owned())
}

impl GritApp {
    pub(super) fn repository_tools_view(&mut self, ui: &mut egui::Ui) {
        ui.heading("Repository tools");
        if self.path.is_none() {
            ui.label("Open a repository to use these tools.");
            return;
        }
        let busy = self.operation.is_some();
        ui.columns(2, |cols| {
            cols[0].group(|ui| {
                ui.heading("Branches");
                ui.label(format!(
                    "Current: {}",
                    if self.branch.is_empty() {
                        "(detached)"
                    } else {
                        &self.branch
                    }
                ));
                ui.horizontal(|ui| {
                    ui.label("Rename");
                    ui.add_sized(
                        [100., 24.],
                        egui::TextEdit::singleline(&mut self.repo_tools.rename_from),
                    );
                    ui.label("to");
                    ui.add_sized(
                        [100., 24.],
                        egui::TextEdit::singleline(&mut self.repo_tools.rename_to),
                    );
                });
                if ui
                    .add_enabled(
                        !busy
                            && !self.repo_tools.rename_from.trim().is_empty()
                            && !self.repo_tools.rename_to.trim().is_empty(),
                        egui::Button::new("Rename local branch"),
                    )
                    .clicked()
                {
                    let old = self.repo_tools.rename_from.trim().to_owned();
                    let new = self.repo_tools.rename_to.trim().to_owned();
                    self.start_git_command(
                        "Rename branch",
                        vec!["branch".into(), "-m".into(), "--".into(), old, new],
                    );
                }
                ui.separator();
                ui.label("Merge a local branch into the current branch");
                egui::ComboBox::from_id_salt("repo_merge_branch")
                    .selected_text(if self.repo_tools.merge_branch.is_empty() {
                        "Select branch"
                    } else {
                        &self.repo_tools.merge_branch
                    })
                    .show_ui(ui, |ui| {
                        for name in self.branches.iter().filter(|b| *b != &self.branch) {
                            ui.selectable_value(
                                &mut self.repo_tools.merge_branch,
                                name.clone(),
                                name,
                            );
                        }
                    });
                if ui
                    .add_enabled(
                        !busy && !self.repo_tools.merge_branch.is_empty(),
                        egui::Button::new("Merge selected branch"),
                    )
                    .clicked()
                {
                    let branch = self.repo_tools.merge_branch.clone();
                    self.start_git_command(
                        "Merge",
                        vec!["merge".into(), "--no-edit".into(), "--".into(), branch],
                    );
                }
            });
            cols[1].group(|ui| {
                ui.heading("Annotated tag");
                ui.horizontal(|ui| {
                    ui.label("Name");
                    ui.text_edit_singleline(&mut self.repo_tools.tag_name);
                });
                ui.horizontal(|ui| {
                    ui.label("Target");
                    ui.text_edit_singleline(&mut self.repo_tools.tag_target);
                });
                ui.add(
                    egui::TextEdit::multiline(&mut self.repo_tools.tag_message)
                        .hint_text("Tag message")
                        .desired_rows(2),
                );
                let tag_ok = !busy
                    && !self.repo_tools.tag_name.trim().is_empty()
                    && !self.repo_tools.tag_message.trim().is_empty();
                if ui
                    .add_enabled(tag_ok, egui::Button::new("Create annotated tag"))
                    .clicked()
                {
                    let mut args = vec![
                        "tag".to_owned(),
                        "-a".into(),
                        "-m".into(),
                        self.repo_tools.tag_message.trim().into(),
                        "--".into(),
                        self.repo_tools.tag_name.trim().into(),
                    ];
                    if !self.repo_tools.tag_target.trim().is_empty() {
                        args.push(self.repo_tools.tag_target.trim().into());
                    }
                    self.start_git_command("Create annotated tag", args);
                }
                ui.separator();
                ui.heading("Delete tag");
                for tag in self.tags.clone() {
                    ui.horizontal(|ui| {
                        ui.label(&tag);
                        if ui
                            .add_enabled(!busy, egui::Button::new("Delete…"))
                            .clicked()
                        {
                            self.repo_tools.pending_tag_delete = Some(tag);
                        }
                    });
                }
                if let Some(tag) = self.repo_tools.pending_tag_delete.clone() {
                    ui.colored_label(
                        Color32::YELLOW,
                        format!("Delete tag {tag}? This cannot be undone."),
                    )
                    .on_hover_text("This explicit confirmation is required before deletion.");
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(!busy, egui::Button::new("Confirm delete tag"))
                            .clicked()
                        {
                            self.git_action("Delete tag", &["tag", "-d", "--", &tag]);
                            self.repo_tools.pending_tag_delete = None;
                        }
                        if ui.button("Cancel").clicked() {
                            self.repo_tools.pending_tag_delete = None;
                        }
                    });
                }
            });
        });
        ui.add_space(6.);
        ui.columns(2, |cols| {
            cols[0].group(|ui| {
                ui.heading("Submodules");
                if let Some(repo) = self.repo() {
                    match repo.submodules() {
                        Ok(modules) if modules.is_empty() => {
                            ui.label("No submodules configured.");
                        }
                        Ok(modules) => {
                            for module in modules {
                                ui.label(format!(
                                    "{}  ({})",
                                    module.path().display(),
                                    module.url().unwrap_or("URL unavailable")
                                ));
                            }
                        }
                        Err(error) => {
                            ui.label(format!("Could not read submodules: {error}"));
                        }
                    }
                }
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(!busy, egui::Button::new("Init and update"))
                        .clicked()
                    {
                        self.start_git_command(
                            "Update submodules",
                            vec!["submodule".into(), "update".into(), "--init".into()],
                        );
                    }
                });
            });
            cols[1].group(|ui| {
                ui.heading("Add to .gitignore");
                ui.horizontal(|ui| {
                    ui.selectable_value(
                        &mut self.repo_tools.ignore_raw_pattern,
                        false,
                        "Literal path",
                    );
                    ui.selectable_value(
                        &mut self.repo_tools.ignore_raw_pattern,
                        true,
                        "Raw pattern",
                    );
                });
                ui.label(if self.repo_tools.ignore_raw_pattern {
                    "Git ignore pattern (wildcards active)."
                } else {
                    "Repository relative path; treated literally."
                });
                ui.text_edit_singleline(&mut self.repo_tools.ignore_path);
                let preview = if self.repo_tools.ignore_raw_pattern {
                    let raw = &self.repo_tools.ignore_path;
                    if raw.is_empty()
                        || raw.contains('\n')
                        || raw.contains('\r')
                        || raw.contains('\0')
                    {
                        Err("Enter a non-empty pattern without line breaks".to_owned())
                    } else {
                        Ok(raw.clone())
                    }
                } else {
                    literal_ignore_pattern(&self.repo_tools.ignore_path)
                };
                match &preview {
                    Ok(p) => {
                        ui.label(format!("Preview: {p}"));
                    }
                    Err(e) => {
                        if !self.repo_tools.ignore_path.is_empty() {
                            ui.colored_label(Color32::YELLOW, e);
                        }
                    }
                }
                if ui
                    .add_enabled(
                        !busy && preview.is_ok(),
                        egui::Button::new("Append pattern to repository .gitignore"),
                    )
                    .clicked()
                    && let Some(root) = self.path.clone()
                {
                    let result = if self.repo_tools.ignore_raw_pattern {
                        append_raw_ignore(&root, &self.repo_tools.ignore_path)
                    } else {
                        append_ignore(&root, &self.repo_tools.ignore_path)
                    };
                    match result {
                        Ok(pattern) => {
                            self.notice = format!("Added {pattern} to repository .gitignore")
                        }
                        Err(error) => self.notice = error,
                    }
                }
            });
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn git(root: &Path, args: &[&str]) -> String {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {:?}: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    }

    #[test]
    fn branch_merge_and_annotated_tag_actions_work_in_a_repository() {
        let root = std::env::temp_dir().join(format!(
            "grit-repo-actions-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        git(&root, &["init", "-q"]);
        git(&root, &["config", "user.name", "Grit Test"]);
        git(&root, &["config", "user.email", "grit@example.invalid"]);
        fs::write(root.join("base.txt"), "base\n").unwrap();
        git(&root, &["add", "base.txt"]);
        git(&root, &["commit", "-qm", "base"]);
        let current = git(&root, &["branch", "--show-current"]);
        git(&root, &["branch", "-m", "--", &current, "renamed"]);
        git(&root, &["branch", "feature"]);
        git(&root, &["checkout", "-q", "feature"]);
        fs::write(root.join("feature.txt"), "feature\n").unwrap();
        git(&root, &["add", "feature.txt"]);
        git(&root, &["commit", "-qm", "feature"]);
        git(&root, &["checkout", "-q", "renamed"]);
        git(&root, &["merge", "--no-edit", "--", "feature"]);
        git(
            &root,
            &["tag", "-a", "-m", "release message", "--", "release-v1"],
        );
        assert_eq!(git(&root, &["branch", "--show-current"]), "renamed");
        assert!(
            git(&root, &["cat-file", "-p", "refs/tags/release-v1"]).contains("release message")
        );
        assert!(root.join("feature.txt").exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn ignore_pattern_escapes_gitignore_metacharacters() {
        assert_eq!(
            literal_ignore_pattern("dir/#bang!/[a]*?.txt ").unwrap(),
            r"/dir/\#bang!/\[a\]\*\?.txt\ "
        );
    }
    #[test]
    fn ignore_pattern_rejects_newline_injection() {
        assert!(literal_ignore_pattern("safe\n/important").is_err());
        assert!(literal_ignore_pattern("safe\r/important").is_err());
    }
    #[test]
    fn appends_without_overwriting_existing_data() {
        let root = std::env::temp_dir().join(format!("grit-ignore-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join(".gitignore"), b"existing\n").unwrap();
        assert_eq!(
            append_ignore(&root, "literal*.dat").unwrap(),
            r"/literal\*.dat"
        );
        assert_eq!(
            fs::read(root.join(".gitignore")).unwrap(),
            b"existing\n/literal\\*.dat\n"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn literal_patterns_match_metacharacters_and_backslash_names_only() {
        let root = std::env::temp_dir().join(format!(
            "grit-ignore-match-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        git(&root, &["init", "-q"]);
        fs::write(root.join("a[b]*.txt"), "literal").unwrap();
        fs::write(root.join("axbZ.txt"), "glob lookalike").unwrap();
        fs::write(root.join("back\\slash.txt"), "backslash literal").unwrap();
        assert_eq!(append_ignore(&root, "a[b]*.txt").unwrap(), r"/a\[b\]\*.txt");
        let backslash_pattern = append_ignore(&root, "back\\slash.txt").unwrap();
        assert_eq!(backslash_pattern, "/back\\\\slash.txt");
        assert!(
            std::process::Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(["check-ignore", "--no-index", "-q", "--", "a[b]*.txt"])
                .status()
                .unwrap()
                .success()
        );
        assert!(
            !std::process::Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(["check-ignore", "--no-index", "-q", "--", "axbZ.txt"])
                .status()
                .unwrap()
                .success()
        );
        assert!(
            std::process::Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(["check-ignore", "--no-index", "-q", "--", "back\\slash.txt"])
                .status()
                .unwrap()
                .success()
        );
        let _ = fs::remove_dir_all(root);
    }
}
