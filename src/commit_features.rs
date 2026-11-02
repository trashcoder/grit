use super::*;
use std::fs;

const MAX_TEMPLATE_BYTES: u64 = 1024 * 1024;

#[derive(Clone, Default)]
pub(super) struct CommitOptions {
    pub template: String,
    pub templates: Vec<(String, String)>,
    pub template_name: String,
    pub author_name: String,
    pub author_email: String,
    pub override_author: bool,
    pub signoff: bool,
    confirm_template_overwrite: bool,
    pending_template_export: Option<(PathBuf, String)>,
    pending_template_import: Option<(String, String)>,
}

pub(super) fn apply_template(
    current: &str,
    template: &str,
    confirmed: bool,
) -> Result<String, String> {
    if !current.trim().is_empty() && !confirmed {
        return Err("Applying this template will replace the current message".into());
    }
    Ok(template.to_owned())
}

pub(super) fn load_amend_message(repo: &Repository) -> Result<String, String> {
    let head = repo.head().map_err(|e| e.to_string())?;
    let commit = head.peel_to_commit().map_err(|e| e.to_string())?;
    commit
        .message()
        .map(str::to_owned)
        .ok_or_else(|| "HEAD commit message is not valid UTF-8".to_owned())
}

fn read_template_file(path: &Path) -> Result<String, String> {
    let meta = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if meta.file_type().is_symlink() || !meta.is_file() {
        return Err("Template must be a regular, non-symlink file".into());
    }
    if meta.len() > MAX_TEMPLATE_BYTES {
        return Err("Template exceeds the 1 MiB size limit".into());
    }
    String::from_utf8(fs::read(path).map_err(|e| e.to_string())?)
        .map_err(|_| "Template is not valid UTF-8".into())
}

fn write_template_file(path: &Path, contents: &str, confirmed: bool) -> Result<(), String> {
    if contents.len() as u64 > MAX_TEMPLATE_BYTES {
        return Err("Template exceeds the 1 MiB size limit".into());
    }
    if let Ok(meta) = fs::symlink_metadata(path) {
        if meta.file_type().is_symlink() {
            return Err("Refusing to overwrite a symbolic link".into());
        }
        if !meta.is_file() {
            return Err("Export destination must be a regular file".into());
        }
        if !confirmed {
            return Err("The destination exists; confirm overwrite first".into());
        }
    }
    fs::write(path, contents).map_err(|e| e.to_string())
}

pub(super) fn load_template(repo: &Repository) -> Result<Option<String>, String> {
    let config = repo.config().map_err(|e| e.to_string())?;
    let configured = match config.get_string("commit.template") {
        Ok(path) => path,
        Err(e) if e.code() == git2::ErrorCode::NotFound => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    let expanded = if configured == "~" {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or("HOME is not set")?
    } else if let Some(rest) = configured.strip_prefix("~/") {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or("HOME is not set")?
            .join(rest)
    } else {
        PathBuf::from(configured)
    };
    let path = if expanded.is_absolute() {
        expanded
    } else {
        let base = repo
            .workdir()
            .or_else(|| repo.path().parent())
            .ok_or("Repository has no base directory")?;
        // Git interprets relative commit.template paths from the repository worktree.
        base.join(expanded)
    };
    let meta =
        fs::symlink_metadata(&path).map_err(|e| format!("Could not read commit template: {e}"))?;
    if meta.file_type().is_symlink() || !meta.is_file() {
        return Err("Commit template must be a regular, non-symlink file".into());
    }
    if meta.len() > MAX_TEMPLATE_BYTES {
        return Err("Commit template exceeds the 1 MiB size limit".into());
    }
    let bytes = fs::read(path).map_err(|e| format!("Could not read commit template: {e}"))?;
    if bytes.len() as u64 > MAX_TEMPLATE_BYTES {
        return Err("Commit template exceeds the 1 MiB size limit".into());
    }
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|_| "Commit template is not valid UTF-8".into())
}

pub(super) fn commit_arguments(
    message: &str,
    amend: bool,
    options: &CommitOptions,
) -> Result<Vec<String>, String> {
    if options.override_author {
        validate_identity_part(&options.author_name, "author name", false)?;
        validate_identity_part(&options.author_email, "author email", true)?;
    }
    let mut args = vec!["commit".to_owned()];
    if amend {
        args.push("--amend".into());
    }
    if options.signoff {
        args.push("--signoff".into());
    }
    if options.override_author {
        args.push("--author".into());
        args.push(format!(
            "{} <{}>",
            options.author_name, options.author_email
        ));
    }
    args.push("-m".into());
    args.push(message.to_owned());
    Ok(args)
}

fn validate_identity_part(value: &str, label: &str, email: bool) -> Result<(), String> {
    let malformed = value.trim().is_empty()
        || value
            .chars()
            .any(|c| c == '\n' || c == '\r' || c == '\0' || c.is_control())
        || value.contains(['<', '>'])
        || (email && value.chars().any(char::is_whitespace));
    if malformed {
        return Err(format!(
            "Invalid {label}: provide a non-empty value without control characters or angle brackets{}",
            if email { " or whitespace" } else { "" }
        ));
    }
    Ok(())
}

impl GritApp {
    pub(super) fn commit_options_ui(&mut self, ui: &mut egui::Ui) {
        let options = &mut self.commit_options;
        if options.template_name.is_empty()
            && let Some((name, value)) = options.templates.first()
        {
            options.template_name = name.clone();
            if options.template.is_empty() {
                options.template = value.clone();
            }
        }
        ui.label("Commit template");
        let mut selected_name = options.template_name.clone();
        egui::ComboBox::from_id_salt("commit_template_choice")
            .selected_text(if selected_name.is_empty() {
                "(manual)"
            } else {
                &selected_name
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut selected_name, String::new(), "(manual)");
                for (name, _) in &options.templates {
                    ui.selectable_value(&mut selected_name, name.clone(), name);
                }
            });
        if selected_name != options.template_name {
            options.template_name = selected_name;
            if let Some((_, value)) = options
                .templates
                .iter()
                .find(|(n, _)| n == &options.template_name)
            {
                options.template = value.clone();
            }
        }
        ui.add(egui::TextEdit::singleline(&mut options.template_name).hint_text("Template name"));
        let changed = ui
            .add(
                egui::TextEdit::multiline(&mut options.template)
                    .desired_rows(3)
                    .hint_text("Template text"),
            )
            .changed();
        if changed
            && let Some((_, value)) = options
                .templates
                .iter_mut()
                .find(|(n, _)| n == &options.template_name)
        {
            *value = options.template.clone();
        }
        ui.horizontal(|ui| {
            if ui.button("Add").clicked() {
                let name = if options.template_name.trim().is_empty() {
                    "Template".to_owned()
                } else {
                    format!("{} copy", options.template_name)
                };
                let unique = unique_template_name(&options.templates, &name);
                options
                    .templates
                    .push((unique.clone(), options.template.clone()));
                options.template_name = unique;
            }
            if ui.button("Replace selected").clicked() {
                if let Some((_, value)) = options
                    .templates
                    .iter_mut()
                    .find(|(n, _)| n == &options.template_name)
                {
                    *value = options.template.clone();
                } else {
                    let name = unique_template_name(
                        &options.templates,
                        if options.template_name.trim().is_empty() {
                            "Template"
                        } else {
                            &options.template_name
                        },
                    );
                    options
                        .templates
                        .push((name.clone(), options.template.clone()));
                    options.template_name = name;
                }
            }
            if ui.button("Remove").clicked() {
                options
                    .templates
                    .retain(|(name, _)| name != &options.template_name);
                options.template_name.clear();
                if let Some((name, value)) = options.templates.first() {
                    options.template_name = name.clone();
                    options.template = value.clone();
                } else {
                    options.template.clear();
                }
            }
            if ui.button("Import .txt…").clicked()
                && let Err(error) = import_template(options)
            {
                self.notice = error;
            }
            if ui.button("Export .txt…").clicked()
                && let Err(error) = export_template(options)
            {
                self.notice = error;
            }
            if ui.button("Apply").clicked() {
                match apply_template(
                    &self.message,
                    &options.template,
                    options.confirm_template_overwrite,
                ) {
                    Ok(message) => {
                        self.message = message;
                        options.confirm_template_overwrite = false;
                    }
                    Err(_) => options.confirm_template_overwrite = true,
                }
            }
        });
        if options.confirm_template_overwrite {
            ui.horizontal(|ui| {
                ui.label("Replace the current commit message?");
                if ui.button("Replace message").clicked() {
                    self.message = options.template.clone();
                    options.confirm_template_overwrite = false;
                }
                if ui.button("Cancel").clicked() {
                    options.confirm_template_overwrite = false;
                }
            });
        }
        ui.separator();
        ui.checkbox(&mut options.override_author, "Override author");
        if options.override_author {
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut options.author_name).hint_text("Name"));
                ui.add(egui::TextEdit::singleline(&mut options.author_email).hint_text("Email"));
            });
        }
        ui.checkbox(&mut options.signoff, "Add sign-off (Signed-off-by)");

        if let Some((path, contents)) = options.pending_template_export.clone() {
            egui::Window::new("Overwrite template file?")
                .collapsible(false)
                .show(ui.ctx(), |ui| {
                    ui.label(format!("Replace {}?", path.display()));
                    if ui.button("Overwrite").clicked() {
                        let result = write_template_file(&path, &contents, true);
                        self.notice = result
                            .map(|_| format!("Exported template to {}", path.display()))
                            .unwrap_or_else(|e| e);
                        options.pending_template_export = None;
                    }
                    if ui.button("Cancel").clicked() {
                        options.pending_template_export = None;
                    }
                });
        }
        if let Some((name, contents)) = options.pending_template_import.clone() {
            egui::Window::new("Replace template?")
                .collapsible(false)
                .show(ui.ctx(), |ui| {
                    ui.label(format!("Replace saved template ‘{name}’?"));
                    if ui.button("Replace template").clicked() {
                        if let Some((_, value)) =
                            options.templates.iter_mut().find(|(n, _)| n == &name)
                        {
                            *value = contents.clone();
                        }
                        options.template_name = name.clone();
                        options.template = contents.clone();
                        options.pending_template_import = None;
                    }
                    if ui.button("Cancel").clicked() {
                        options.pending_template_import = None;
                    }
                });
        }
    }
}

fn unique_template_name(templates: &[(String, String)], base: &str) -> String {
    let mut candidate = base.to_owned();
    let mut suffix = 2;
    while templates.iter().any(|(name, _)| name == &candidate) {
        candidate = format!("{base} {suffix}");
        suffix += 1;
    }
    candidate
}

fn import_template(options: &mut CommitOptions) -> Result<(), String> {
    let Some(path) = rfd::FileDialog::new()
        .add_filter("Text", &["txt"])
        .pick_file()
    else {
        return Ok(());
    };
    let contents = read_template_file(&path)?;
    let name = path
        .file_stem()
        .and_then(|v| v.to_str())
        .unwrap_or("Imported template")
        .to_owned();
    if options
        .templates
        .iter()
        .any(|(existing, _)| existing == &name)
    {
        options.pending_template_import = Some((name, contents));
    } else {
        options.templates.push((name.clone(), contents.clone()));
        options.template_name = name;
        options.template = contents;
    }
    Ok(())
}

fn export_template(options: &mut CommitOptions) -> Result<(), String> {
    let stem = if options.template_name.trim().is_empty() {
        "commit-template"
    } else {
        &options.template_name
    };
    let Some(path) = rfd::FileDialog::new()
        .add_filter("Text", &["txt"])
        .set_file_name(format!("{stem}.txt"))
        .save_file()
    else {
        return Ok(());
    };
    match fs::symlink_metadata(&path) {
        Ok(meta) if meta.file_type().is_symlink() => {
            return Err("Refusing to export template over a symbolic link".into());
        }
        Ok(meta) if !meta.is_file() => {
            return Err("Export destination must be a regular file".into());
        }
        Ok(_) => options.pending_template_export = Some((path, options.template.clone())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            write_template_file(&path, &options.template, false)?;
        }
        Err(error) => return Err(error.to_string()),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        process::Command,
        time::{SystemTime, UNIX_EPOCH},
    };

    fn temp_repo() -> (PathBuf, Repository) {
        let dir = std::env::temp_dir().join(format!(
            "grit-commit-features-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        let repo = Repository::init(&dir).unwrap();
        let mut config = repo.config().unwrap();
        config.set_str("user.name", "Configured User").unwrap();
        config
            .set_str("user.email", "configured@example.test")
            .unwrap();
        (dir, repo)
    }

    #[test]
    fn template_loading_and_commit_author_signoff() {
        let (dir, repo) = temp_repo();
        fs::write(dir.join("TEMPLATE"), "Template subject\n\nBody\n").unwrap();
        repo.config()
            .unwrap()
            .set_str("commit.template", "TEMPLATE")
            .unwrap();
        assert_eq!(
            load_template(&repo).unwrap().as_deref(),
            Some("Template subject\n\nBody\n")
        );
        let opts = CommitOptions {
            override_author: true,
            author_name: "Other Author".into(),
            author_email: "other@example.test".into(),
            signoff: true,
            ..Default::default()
        };
        let args = commit_arguments("commit body", false, &opts).unwrap();
        fs::write(dir.join("file"), "one").unwrap();
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(&dir)
                .args(["add", "file"])
                .status()
                .unwrap()
                .success()
        );
        let output = Command::new("git")
            .arg("-C")
            .arg(&dir)
            .args(&args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            args.windows(2)
                .any(|w| w[0] == "--author" && w[1] == "Other Author <other@example.test>")
        );
        assert!(args.contains(&"--signoff".to_owned()));
        let commit = repo.head().unwrap().peel_to_commit().unwrap();
        assert_eq!(commit.author().name(), Some("Other Author"));
        assert!(
            commit
                .message()
                .unwrap()
                .contains("Signed-off-by: Configured User <configured@example.test>")
        );
        assert_eq!(
            load_amend_message(&repo).unwrap(),
            "commit body\n\nSigned-off-by: Configured User <configured@example.test>\n"
        );
        fs::write(dir.join("file"), "two").unwrap();
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(&dir)
                .args(["add", "file"])
                .status()
                .unwrap()
                .success()
        );
        let amend_args = commit_arguments("amended body", true, &opts).unwrap();
        let amended = Command::new("git")
            .arg("-C")
            .arg(&dir)
            .args(&amend_args)
            .output()
            .unwrap();
        assert!(
            amended.status.success(),
            "{}",
            String::from_utf8_lossy(&amended.stderr)
        );
        assert!(amend_args.contains(&"--amend".to_owned()));
        assert_eq!(
            repo.head()
                .unwrap()
                .peel_to_commit()
                .unwrap()
                .parent_count(),
            0
        );
        let mut bad = opts.clone();
        bad.author_name.push('\n');
        assert!(commit_arguments("x", false, &bad).is_err());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn template_application_requires_confirmation_and_preserves_text() {
        assert!(apply_template("existing message", "new template", false).is_err());
        assert_eq!(
            apply_template("existing message", "new template", true).unwrap(),
            "new template"
        );
        assert_eq!(
            apply_template("", "new template", false).unwrap(),
            "new template"
        );
    }
}
