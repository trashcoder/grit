use super::*;

#[derive(Clone, PartialEq, Eq)]
struct TagForm {
    name: String,
    revision: String,
    annotated: bool,
    message: String,
}
impl Default for TagForm {
    fn default() -> Self {
        Self {
            name: String::new(),
            revision: "HEAD".into(),
            annotated: true,
            message: String::new(),
        }
    }
}
#[derive(Clone)]
struct TagReview {
    repo: PathBuf,
    form: TagForm,
    target: git2::Oid,
    expected: Option<git2::Oid>,
}
#[derive(Default)]
pub(super) struct VersionTagTools {
    template: String,
    form: TagForm,
    pending: Option<TagReview>,
    acknowledge_replace: bool,
}
fn valid_tag_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('-')
        && git2::Reference::is_valid_name(&format!("refs/tags/{name}"))
}
fn suggest_version_name(name: &str) -> String {
    let end = name.len();
    let start = name
        .char_indices()
        .rev()
        .take_while(|(_, ch)| ch.is_ascii_digit())
        .last()
        .map(|(i, _)| i);
    if let Some(start) = start
        && let Ok(number) = name[start..end].parse::<u64>()
        && let Some(next) = number.checked_add(1)
    {
        return format!("{}{:0width$}", &name[..start], next, width = end - start);
    }
    format!("{name}-new")
}
fn resolve_target(repo: &Repository, revision: &str) -> Result<git2::Oid, String> {
    repo.revparse_single(revision)
        .and_then(|o| o.peel_to_commit())
        .map(|c| c.id())
        .map_err(|e| e.to_string())
}
fn current_tag(repo: &Repository, name: &str) -> Result<Option<git2::Oid>, String> {
    match repo.find_reference(&format!("refs/tags/{name}")) {
        Ok(reference) => reference
            .target()
            .map(Some)
            .ok_or_else(|| "Symbolic tags cannot be replaced here".into()),
        Err(error) if error.code() == git2::ErrorCode::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}
fn prepare_tag(repo_path: &Path, repo: &Repository, form: &TagForm) -> Result<TagReview, String> {
    if !valid_tag_name(&form.name) {
        return Err("Enter a valid tag name".into());
    }
    if form.annotated && form.message.trim().is_empty() {
        return Err("An annotated tag needs a message".into());
    }
    if form.message.contains('\0') {
        return Err("Tag messages cannot contain NUL".into());
    }
    Ok(TagReview {
        repo: repo_path.to_path_buf(),
        form: form.clone(),
        target: resolve_target(repo, &form.revision)?,
        expected: current_tag(repo, &form.name)?,
    })
}
fn apply_tag(review: &TagReview) -> Result<String, String> {
    let repo = Repository::open(&review.repo).map_err(|e| e.to_string())?;
    if resolve_target(&repo, &review.form.revision)? != review.target {
        return Err("The target revision changed. Review the tag again".into());
    }
    if current_tag(&repo, &review.form.name)? != review.expected {
        return Err("The existing tag changed. Review it again".into());
    }
    let new_oid = if review.form.annotated {
        let target = repo
            .find_object(review.target, None)
            .map_err(|e| e.to_string())?;
        repo.tag_annotation_create(
            &review.form.name,
            &target,
            &repo.signature().map_err(|e| e.to_string())?,
            &review.form.message,
        )
        .map_err(|e| e.to_string())?
    } else {
        review.target
    };
    let reference = format!("refs/tags/{}", review.form.name);
    match review.expected {
        Some(expected) => {
            repo.reference_matching(
                &reference,
                new_oid,
                true,
                expected,
                "grit: replace reviewed tag",
            )
            .map_err(|e| e.to_string())?;
        }
        None => {
            repo.reference(&reference, new_oid, false, "grit: create version tag")
                .map_err(|e| e.to_string())?;
        }
    }
    Ok(format!(
        "{} tag {} at {}",
        if review.expected.is_some() {
            "Replaced"
        } else {
            "Created"
        },
        review.form.name,
        review.target
    ))
}
#[derive(Clone)]
pub(super) struct TagDeletion {
    pub name: String,
    pub expected: String,
}
pub(super) fn review_tag_deletion(repo: &Repository, name: &str) -> Result<TagDeletion, String> {
    if !valid_tag_name(name) {
        return Err("Invalid tag name".into());
    }
    let expected = current_tag(repo, name)?.ok_or_else(|| "The tag no longer exists".to_owned())?;
    Ok(TagDeletion {
        name: name.to_owned(),
        expected: expected.to_string(),
    })
}
impl TagDeletion {
    pub fn arguments(&self) -> Vec<String> {
        vec![
            "update-ref".into(),
            "--no-deref".into(),
            "-d".into(),
            format!("refs/tags/{}", self.name),
            self.expected.clone(),
        ]
    }
}
impl GritApp {
    pub(super) fn version_tags_ui(&mut self, ui: &mut egui::Ui) {
        egui::CollapsingHeader::new("Version tags and reviewed replacement").show(ui,|ui| {
            let Some(repo) = self.repo() else { return; };
            let Some(path) = self.path.clone() else { return; };
            let busy = self.operation.is_some();
            ui.horizontal_wrapped(|ui| {
                egui::ComboBox::from_id_salt("version_tag_template").selected_text(if self.version_tags.template.is_empty(){"Existing tag as template"}else{&self.version_tags.template}).show_ui(ui,|ui| {
                    for tag in &self.tags { ui.selectable_value(&mut self.version_tags.template,tag.clone(),tag); }
                });
                if ui.add_enabled(!busy && !self.version_tags.template.is_empty(),egui::Button::new("Use template")).clicked() {
                    self.version_tags.form.name = suggest_version_name(&self.version_tags.template);
                    if let Ok(object) = repo.revparse_single(&format!("refs/tags/{}",self.version_tags.template)) {
                        self.version_tags.form.annotated = object.as_tag().is_some();
                        self.version_tags.form.message = object.as_tag().and_then(|t|t.message()).unwrap_or_default().into();
                    }
                    self.version_tags.form.revision = self.selected_commit.clone().unwrap_or_else(||"HEAD".into());
                }
            });
            ui.label("A trailing version number is incremented as a suggestion; edit the proposed name before creating.");
            ui.horizontal_wrapped(|ui| {
                ui.label("Tag name");ui.text_edit_singleline(&mut self.version_tags.form.name);
                ui.label("Target revision");ui.text_edit_singleline(&mut self.version_tags.form.revision);
            });
            ui.checkbox(&mut self.version_tags.form.annotated,"Annotated tag");
            if self.version_tags.form.annotated { ui.add(egui::TextEdit::multiline(&mut self.version_tags.form.message).desired_rows(2).hint_text("Tag message")); }
            if ui.add_enabled(!busy,egui::Button::new("Review tag…")).clicked() {
                match prepare_tag(&path,&repo,&self.version_tags.form) {
                    Ok(review) => {self.version_tags.pending=Some(review);self.version_tags.acknowledge_replace=false;}
                    Err(error) => self.notice=error,
                }
            }
            if self.version_tags.pending.as_ref().is_some_and(|review| review.repo!=path || review.form!=self.version_tags.form || current_tag(&repo,&review.form.name).ok()!=Some(review.expected) || resolve_target(&repo,&review.form.revision).ok()!=Some(review.target)) {
                self.version_tags.pending=None;
                ui.colored_label(Color32::YELLOW,"Tag review expired because its inputs or references changed");
            }
            if let Some(review)=self.version_tags.pending.clone() {
                ui.label(format!("{} → commit {}",review.form.name,review.target));
                if let Some(old)=review.expected {
                    ui.label(format!("Existing tag object: {old}. Its name will point to the reviewed replacement."));
                    ui.checkbox(&mut self.version_tags.acknowledge_replace,"Replace this existing local tag");
                }
                ui.horizontal_wrapped(|ui| {
                    if ui.add_enabled(!busy && (review.expected.is_none() || self.version_tags.acknowledge_replace),egui::Button::new("Confirm tag update")).clicked() {
                        let (tx,rx)=mpsc::channel();
                        std::thread::spawn(move||{let _=tx.send(apply_tag(&review));});
                        self.operation=Some(("Update version tag".into(),rx));self.version_tags.pending=None;
                    }
                    if ui.button("Cancel").clicked(){self.version_tags.pending=None;}
                });
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    #[test]
    fn version_suggestions_handle_width_overflow_unicode_and_plain_names() {
        assert_eq!(suggest_version_name("v1.2.009"), "v1.2.010");
        assert_eq!(suggest_version_name("β-release9"), "β-release10");
        assert_eq!(suggest_version_name("stable"), "stable-new");
        assert_eq!(
            suggest_version_name("v18446744073709551615"),
            "v18446744073709551615-new"
        );
        assert!(!valid_tag_name("-bad"));
        assert!(!valid_tag_name("v1:bad"));
    }
    #[test]
    fn creates_annotated_tags_and_rejects_stale_replacements_without_touching_index() {
        let root = std::env::temp_dir().join(format!(
            "grit-version-tags-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let repo = Repository::init(&root).unwrap();
        repo.config()
            .unwrap()
            .set_str("user.name", "Tag test")
            .unwrap();
        repo.config()
            .unwrap()
            .set_str("user.email", "tag@example.test")
            .unwrap();
        fs::write(root.join("file"), "first").unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("file")).unwrap();
        index.write().unwrap();
        let tree_id = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let sig = repo.signature().unwrap();
        let first = repo
            .commit(Some("HEAD"), &sig, &sig, "first", &tree, &[])
            .unwrap();
        let parent = repo.find_commit(first).unwrap();
        let second = repo
            .commit(Some("HEAD"), &sig, &sig, "second", &tree, &[&parent])
            .unwrap();
        fs::write(root.join("file"), "unstaged").unwrap();
        let before = fs::read(repo.path().join("index")).unwrap();
        let form = TagForm {
            name: "v1.2.3".into(),
            revision: first.to_string(),
            annotated: true,
            message: "release message".into(),
        };
        let review = prepare_tag(&root, &repo, &form).unwrap();
        apply_tag(&review).unwrap();
        let tag = repo.revparse_single("refs/tags/v1.2.3").unwrap();
        assert_eq!(tag.as_tag().unwrap().message(), Some("release message"));
        assert_eq!(tag.peel_to_commit().unwrap().id(), first);
        assert!(
            apply_tag(&review).is_err(),
            "creating a new tag must not overwrite a concurrent ref"
        );
        let next = TagForm {
            revision: second.to_string(),
            annotated: false,
            ..form
        };
        let review = prepare_tag(&root, &repo, &next).unwrap();
        repo.reference("refs/tags/v1.2.3", first, true, "concurrent")
            .unwrap();
        assert!(apply_tag(&review).is_err());
        let review = prepare_tag(&root, &repo, &next).unwrap();
        apply_tag(&review).unwrap();
        assert_eq!(repo.refname_to_id("refs/tags/v1.2.3").unwrap(), second);
        let deletion = review_tag_deletion(&repo, "v1.2.3").unwrap();
        repo.reference("refs/tags/v1.2.3", first, true, "concurrent replacement")
            .unwrap();
        let failed = Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(deletion.arguments())
            .output()
            .unwrap();
        assert!(!failed.status.success());
        assert_eq!(repo.refname_to_id("refs/tags/v1.2.3").unwrap(), first);
        let deletion = review_tag_deletion(&repo, "v1.2.3").unwrap();
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(deletion.arguments())
                .output()
                .unwrap()
                .status
                .success()
        );
        assert!(current_tag(&repo, "v1.2.3").unwrap().is_none());
        assert_eq!(fs::read(repo.path().join("index")).unwrap(), before);
        assert_eq!(fs::read_to_string(root.join("file")).unwrap(), "unstaged");
        let head_form = TagForm {
            name: "moving".into(),
            revision: "HEAD".into(),
            annotated: false,
            message: String::new(),
        };
        let review = prepare_tag(&root, &repo, &head_form).unwrap();
        repo.reference("refs/heads/moving", first, true, "move")
            .unwrap();
        repo.set_head("refs/heads/moving").unwrap();
        assert!(apply_tag(&review).is_err());
        let _ = fs::remove_dir_all(root);
    }
}
