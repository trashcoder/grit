use super::*;
use chrono::{DateTime, FixedOffset};

#[derive(Clone, Debug, PartialEq, Eq)]
struct Identity {
    name: String,
    email: String,
    date: String,
}

#[derive(Clone, Debug)]
struct AmendDraft {
    head: String,
    head_ref: Option<String>,
    message: String,
    old_author: Identity,
    old_committer: Identity,
    author: Identity,
    committer: Identity,
    confirm: bool,
}

#[derive(Default)]
pub(super) struct MetadataTools {
    draft: Option<AmendDraft>,
}

impl GritApp {
    pub(super) fn metadata_tools_ui(&mut self, ui: &mut egui::Ui) {
        let busy = self.operation.is_some();
        ui.separator();
        ui.heading("Amend commit metadata");
        ui.label("Replace the HEAD author, committer, or their timestamps while preserving its message and tree.");
        if self.metadata_tools.draft.is_none() {
            if ui
                .add_enabled(!busy, egui::Button::new("Edit HEAD metadata…"))
                .clicked()
            {
                self.load_metadata_draft();
            }
            return;
        }

        let Some(draft) = self.metadata_tools.draft.as_mut() else {
            return;
        };
        ui.label(format!("HEAD {}", &draft.head[..draft.head.len().min(12)]));
        ui.label("Original complete commit message");
        ui.add(
            egui::TextEdit::multiline(&mut draft.message)
                .desired_rows(3)
                .interactive(false),
        );
        identity_editor(ui, "Author", &mut draft.author);
        identity_editor(ui, "Committer", &mut draft.committer);

        if draft.confirm {
            ui.separator();
            ui.label("Confirm replacement:");
            ui.label(format!(
                "Author: {} <{}> ({}) → {} <{}> ({})",
                draft.old_author.name,
                draft.old_author.email,
                draft.old_author.date,
                draft.author.name,
                draft.author.email,
                draft.author.date
            ));
            ui.label(format!(
                "Committer: {} <{}> ({}) → {} <{}> ({})",
                draft.old_committer.name,
                draft.old_committer.email,
                draft.old_committer.date,
                draft.committer.name,
                draft.committer.email,
                draft.committer.date
            ));
        }
        let valid =
            validate_identity(&draft.author).is_ok() && validate_identity(&draft.committer).is_ok();
        let mut start = None;
        let mut cancel = false;
        ui.horizontal(|ui| {
            if !draft.confirm {
                if ui
                    .add_enabled(!busy && valid, egui::Button::new("Review changes…"))
                    .clicked()
                {
                    draft.confirm = true;
                }
            } else {
                if ui
                    .add_enabled(!busy && valid, egui::Button::new("Amend HEAD"))
                    .clicked()
                {
                    start = Some(draft.clone());
                }
                if ui.button("Back").clicked() {
                    draft.confirm = false;
                }
            }
            if ui.add_enabled(!busy, egui::Button::new("Cancel")).clicked() {
                cancel = true;
            }
        });
        if !valid {
            ui.colored_label(
                Color32::LIGHT_RED,
                "Enter valid names, emails, and ISO-8601 dates including an offset.",
            );
        }
        if let Some(draft) = start {
            self.start_metadata_amend(draft);
        } else if cancel {
            self.metadata_tools.draft = None;
        }
    }

    fn load_metadata_draft(&mut self) {
        if self.operation.is_some() {
            return;
        }
        let Some(repo) = self.repo() else {
            self.notice = "Open a repository first".into();
            return;
        };
        let result: Result<AmendDraft, String> = (|| {
            let commit = repo
                .head()
                .map_err(|e| e.to_string())?
                .peel_to_commit()
                .map_err(|e| e.to_string())?;
            let author = identity_from_signature(&commit.author())?;
            let committer = identity_from_signature(&commit.committer())?;
            let message = commit.message_bytes();
            let message = String::from_utf8(message.to_vec())
                .map_err(|_| "HEAD message is not valid UTF-8".to_owned())?;
            Ok(AmendDraft {
                head: commit.id().to_string(),
                head_ref: repo
                    .head()
                    .ok()
                    .and_then(|head| head.name().map(str::to_owned)),
                message,
                old_author: author.clone(),
                old_committer: committer.clone(),
                author,
                committer,
                confirm: false,
            })
        })();
        match result {
            Ok(draft) => self.metadata_tools.draft = Some(draft),
            Err(e) => self.notice = format!("Could not read HEAD: {e}"),
        }
    }

    fn start_metadata_amend(&mut self, draft: AmendDraft) {
        if self.operation.is_some() {
            return;
        }
        let Some(path) = self.path.clone() else {
            self.notice = "Open a repository first".into();
            return;
        };
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let result = amend_metadata(&path, &draft);
            let _ = tx.send(result);
        });
        self.notice = "Updating HEAD metadata…".into();
        self.operation = Some(("Update commit metadata".into(), rx));
        self.metadata_tools.draft = None;
    }
}

fn identity_editor(ui: &mut egui::Ui, label: &str, identity: &mut Identity) {
    ui.label(label);
    ui.horizontal(|ui| {
        ui.add(egui::TextEdit::singleline(&mut identity.name).hint_text("Name"));
        ui.add(egui::TextEdit::singleline(&mut identity.email).hint_text("email"));
    });
    ui.add(egui::TextEdit::singleline(&mut identity.date).hint_text("YYYY-MM-DDTHH:MM:SS+HH:MM"));
}

fn identity_from_signature(signature: &git2::Signature<'_>) -> Result<Identity, String> {
    let time = signature.when();
    let offset = FixedOffset::east_opt(time.offset_minutes() * 60)
        .ok_or("Invalid signature timezone offset")?;
    let date = DateTime::from_timestamp(time.seconds(), 0)
        .ok_or("Invalid signature timestamp")?
        .with_timezone(&offset)
        .to_rfc3339();
    Ok(Identity {
        name: signature.name().unwrap_or_default().to_owned(),
        email: signature.email().unwrap_or_default().to_owned(),
        date,
    })
}

fn validate_identity(identity: &Identity) -> Result<(DateTime<FixedOffset>, String), String> {
    for (value, label, email) in [
        (&identity.name, "name", false),
        (&identity.email, "email", true),
    ] {
        if value.trim().is_empty()
            || value
                .chars()
                .any(|c| c == '\n' || c == '\r' || c == '\0' || c.is_control())
            || value.contains(['<', '>'])
            || (email && value.chars().any(char::is_whitespace))
        {
            return Err(format!("Invalid {label}"));
        }
    }
    let date = DateTime::parse_from_rfc3339(&identity.date)
        .map_err(|_| "Date must be ISO-8601 with an explicit offset".to_owned())?;
    Ok((date, identity.date.clone()))
}

fn amend_metadata(path: &Path, draft: &AmendDraft) -> Result<String, String> {
    let repo = Repository::open(path).map_err(|e| e.to_string())?;
    let head = repo
        .head()
        .map_err(|e| e.to_string())?
        .peel_to_commit()
        .map_err(|e| e.to_string())?;
    if head.id().to_string() != draft.head {
        return Err(
            "HEAD changed since the dialog was opened; reload metadata and review again".into(),
        );
    }
    let head_ref = repo
        .head()
        .ok()
        .and_then(|head| head.name().map(str::to_owned));
    if head_ref != draft.head_ref {
        return Err(
            "HEAD reference changed since the dialog was opened; reload metadata and review again"
                .into(),
        );
    }
    if repo.state() != git2::RepositoryState::Clean {
        return Err("Repository is in the middle of another Git operation".into());
    }
    validate_identity(&draft.author)?;
    validate_identity(&draft.committer)?;
    let mut cmd = Command::new("git");
    cmd.arg("-C")
        .arg(path)
        .args(["commit", "--amend", "--only", "--no-edit", "--reset-author"]);
    for (prefix, identity) in [
        ("GIT_AUTHOR", &draft.author),
        ("GIT_COMMITTER", &draft.committer),
    ] {
        let (date, _) = validate_identity(identity)?;
        let date = date.to_rfc3339();
        cmd.env(format!("{prefix}_NAME"), &identity.name)
            .env(format!("{prefix}_EMAIL"), &identity.email)
            .env(format!("{prefix}_DATE"), date);
    }
    let output = cmd.output().map_err(|e| e.to_string())?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    fn temp_repo() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "grit-metadata-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&path).unwrap();
        let run = |args: &[&str]| {
            Command::new("git")
                .arg("-C")
                .arg(&path)
                .args(args)
                .output()
                .unwrap()
        };
        assert!(
            Command::new("git")
                .args(["init", "-q", path.to_str().unwrap()])
                .status()
                .unwrap()
                .success()
        );
        assert!(
            run(&["config", "user.name", "Initial Author"])
                .status
                .success()
        );
        assert!(
            run(&["config", "user.email", "initial@example.test"])
                .status
                .success()
        );
        fs::write(path.join("tracked.txt"), "original\n").unwrap();
        assert!(run(&["add", "tracked.txt"]).status.success());
        assert!(
            run(&["commit", "-q", "-m", "keep this message"])
                .status
                .success()
        );
        path
    }

    #[test]
    fn amend_changes_metadata_preserves_tree_message_and_index() {
        let path = temp_repo();
        let run = |args: &[&str]| {
            Command::new("git")
                .arg("-C")
                .arg(&path)
                .args(args)
                .output()
                .unwrap()
        };
        fs::write(path.join("tracked.txt"), "staged content\n").unwrap();
        assert!(run(&["add", "tracked.txt"]).status.success());
        let staged_before = run(&["write-tree"]);
        let repo = Repository::open(&path).unwrap();
        let commit = repo.head().unwrap().peel_to_commit().unwrap();
        let old_author = identity_from_signature(&commit.author()).unwrap();
        let old_committer = identity_from_signature(&commit.committer()).unwrap();
        let draft = AmendDraft {
            head: commit.id().to_string(),
            head_ref: Repository::open(&path)
                .unwrap()
                .head()
                .ok()
                .and_then(|head| head.name().map(str::to_owned)),
            message: commit.message().unwrap().into(),
            old_author,
            old_committer,
            author: Identity {
                name: "Updated Author".into(),
                email: "author@example.test".into(),
                date: "2024-01-02T03:04:05+02:00".into(),
            },
            committer: Identity {
                name: "Updated Committer".into(),
                email: "committer@example.test".into(),
                date: "2024-02-03T04:05:06-05:00".into(),
            },
            confirm: true,
        };
        amend_metadata(&path, &draft).unwrap();
        let repo = Repository::open(&path).unwrap();
        let new = repo.head().unwrap().peel_to_commit().unwrap();
        assert_eq!(new.message().unwrap(), "keep this message\n");
        assert_eq!(new.tree_id(), commit.tree_id());
        assert_eq!(new.author().name(), Some("Updated Author"));
        assert_eq!(new.author().email(), Some("author@example.test"));
        assert_eq!(new.committer().email(), Some("committer@example.test"));
        assert_eq!(
            new.author().when().seconds(),
            DateTime::parse_from_rfc3339("2024-01-02T03:04:05+02:00")
                .unwrap()
                .timestamp()
        );
        assert_eq!(new.author().when().offset_minutes(), 120);
        assert_eq!(
            new.committer().when().seconds(),
            DateTime::parse_from_rfc3339("2024-02-03T04:05:06-05:00")
                .unwrap()
                .timestamp()
        );
        assert_eq!(new.committer().when().offset_minutes(), -300);
        assert_eq!(run(&["write-tree"]).stdout, staged_before.stdout);
        let _ = fs::remove_dir_all(path);
    }
}
