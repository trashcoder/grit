use super::*;
use std::{fs, io::Write};

const MAX_PREVIEW_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, Default)]
pub(super) struct FileTools {
    pub path: String,
    pub revisions: Vec<FileRevision>,
    pub selected: usize,
    pub bytes: Vec<u8>,
    pub error: Option<String>,
    pending_export: Option<(PathBuf, git2::Oid)>,
}

#[derive(Clone)]
pub(super) struct FileRevision {
    pub id: String,
    pub summary: String,
    pub date: String,
    pub kind: &'static str,
    blob: Option<git2::Oid>,
}

pub(super) fn validate_repo_path(path: &str) -> Result<(), String> {
    let p = Path::new(path);
    if path.is_empty()
        || p.is_absolute()
        || p.components().any(|c| {
            matches!(
                c,
                std::path::Component::ParentDir
                    | std::path::Component::RootDir
                    | std::path::Component::Prefix(_)
            )
        })
    {
        return Err("File path must be a relative path inside the repository".into());
    }
    Ok(())
}

pub(super) fn file_history(repo: &Repository, path: &str) -> Result<Vec<FileRevision>, String> {
    validate_repo_path(path)?;
    let mut walk = repo.revwalk().map_err(|e| e.to_string())?;
    walk.set_sorting(git2::Sort::TOPOLOGICAL | git2::Sort::TIME)
        .map_err(|e| e.to_string())?;
    walk.push_head().map_err(|e| e.to_string())?;
    let mut result = Vec::new();
    for oid in walk {
        let oid = oid.map_err(|e| e.to_string())?;
        let commit = repo.find_commit(oid).map_err(|e| e.to_string())?;
        let tree = commit.tree().map_err(|e| e.to_string())?;
        let parent_tree = if commit.parent_count() == 0 {
            None
        } else {
            Some(
                commit
                    .parent(0)
                    .map_err(|e| e.to_string())?
                    .tree()
                    .map_err(|e| e.to_string())?,
            )
        };
        let mut opts = git2::DiffOptions::new();
        opts.include_typechange(true).include_typechange_trees(true);
        let diff = repo
            .diff_tree_to_tree(parent_tree.as_ref(), Some(&tree), Some(&mut opts))
            .map_err(|e| e.to_string())?;
        for delta in diff.deltas() {
            let old_path = delta.old_file().path().and_then(|p| p.to_str());
            let new_path = delta.new_file().path().and_then(|p| p.to_str());
            if old_path != Some(path) && new_path != Some(path) {
                continue;
            }
            let (blob, kind) = match delta.status() {
                git2::Delta::Deleted => (Some(delta.old_file().id()), "Deleted"),
                git2::Delta::Added | git2::Delta::Copied => (Some(delta.new_file().id()), "Added"),
                git2::Delta::Renamed => {
                    if new_path == Some(path) {
                        (Some(delta.new_file().id()), "Renamed in")
                    } else {
                        (Some(delta.old_file().id()), "Renamed away")
                    }
                }
                _ => (Some(delta.new_file().id()), "Updated"),
            };
            result.push(FileRevision {
                id: oid.to_string(),
                summary: commit.summary().unwrap_or("(no commit message)").to_owned(),
                date: chrono::DateTime::<chrono::Utc>::from_timestamp(commit.time().seconds(), 0)
                    .map(|date| date.format("%Y-%m-%d %H:%M").to_string())
                    .unwrap_or_else(|| "Unknown date".into()),
                kind,
                blob: blob.filter(|id| !id.is_zero()),
            });
            break;
        }
    }
    Ok(result)
}

pub(super) fn load_blob(repo: &Repository, id: git2::Oid) -> Result<Vec<u8>, String> {
    repo.find_blob(id)
        .map(|blob| blob.content().to_vec())
        .map_err(|e| e.to_string())
}

fn write_export(path: &Path, data: &[u8], overwrite_confirmed: bool) -> Result<(), String> {
    if let Ok(meta) = fs::symlink_metadata(path) {
        if meta.file_type().is_symlink() {
            return Err("Refusing to export over a symbolic link".into());
        }
        if !meta.is_file() {
            return Err("Export destination must be a regular file".into());
        }
        if !overwrite_confirmed {
            return Err("The destination already exists; confirm overwrite first".into());
        }
    }
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)
        .map_err(|e| e.to_string())?;
    file.write_all(data).map_err(|e| e.to_string())
}

impl GritApp {
    pub(super) fn open_file_history(&mut self, path: String) {
        self.file_tools.path = path.clone();
        self.file_tools.revisions.clear();
        self.file_tools.selected = 0;
        self.file_tools.bytes.clear();
        self.file_tools.pending_export = None;
        self.file_tools.error = None;
        let result = self
            .repo()
            .ok_or_else(|| "Open a repository first".to_owned())
            .and_then(|repo| file_history(&repo, &path));
        match result {
            Ok(revisions) => {
                self.file_tools.revisions = revisions;
                self.load_file_revision(0);
            }
            Err(error) => self.file_tools.error = Some(error),
        }
        self.tab = 7;
    }

    fn load_file_revision(&mut self, index: usize) {
        let Some(revision) = self.file_tools.revisions.get(index) else {
            return;
        };
        self.file_tools.selected = index;
        let Some(blob) = revision.blob else {
            self.file_tools.bytes.clear();
            return;
        };
        self.file_tools.bytes = self
            .repo()
            .and_then(|repo| load_blob(&repo, blob).ok())
            .unwrap_or_default();
    }

    pub(super) fn file_history_view(&mut self, ui: &mut egui::Ui) {
        ui.heading(format!("File history · {}", self.file_tools.path));
        if let Some(error) = &self.file_tools.error {
            ui.colored_label(Color32::LIGHT_RED, error);
        }
        ui.columns(2, |cols| {
            egui::ScrollArea::vertical().id_salt("file_revision_list").show(&mut cols[0], |ui| {
                let mut picked = None;
                for (i, rev) in self.file_tools.revisions.iter().enumerate() {
                    if ui.selectable_label(i == self.file_tools.selected, format!("{}  {}\n{}\n{}", rev.kind, &rev.id[..8.min(rev.id.len())], rev.summary, rev.date)).clicked() { picked = Some(i); }
                }
                if let Some(i) = picked { self.load_file_revision(i); }
            });
            cols[1].horizontal(|ui| {
                if ui.button("Previous").clicked() && self.file_tools.selected + 1 < self.file_tools.revisions.len() { self.load_file_revision(self.file_tools.selected + 1); }
                if ui.button("Next").clicked() && self.file_tools.selected > 0 { self.load_file_revision(self.file_tools.selected - 1); }
                if ui.button("Export revision…").clicked() { self.begin_file_export(); }
            });
            if self.file_tools.bytes.len() > MAX_PREVIEW_BYTES {
                cols[1].label("Preview unavailable: file exceeds 4 MiB. Export the revision to inspect it.");
            } else if self.file_tools.bytes.contains(&0) {
                cols[1].label("Binary file; preview is unavailable. Export the revision to save the exact blob contents.");
            } else {
                let mut text = String::from_utf8_lossy(&self.file_tools.bytes).into_owned();
                egui::ScrollArea::both().id_salt("file_revision_content").show(&mut cols[1], |ui| { ui.add(egui::TextEdit::multiline(&mut text).code_editor().desired_width(f32::INFINITY)); });
            }
        });
        if let Some((path, blob)) = self.file_tools.pending_export.clone() {
            let exists = fs::symlink_metadata(&path).is_ok();
            egui::Window::new("Confirm overwrite")
                .collapsible(false)
                .show(ui.ctx(), |ui| {
                    ui.label(format!("Replace existing file {}?", path.display()));
                    if ui.button("Overwrite").clicked() {
                        let result = self.export_blob_to(&path, blob, true);
                        self.notice = result.unwrap_or_else(|e| e);
                        self.file_tools.pending_export = None;
                    }
                    if ui.button("Cancel").clicked() {
                        self.file_tools.pending_export = None;
                    }
                });
            if !exists {
                self.file_tools.pending_export = None;
            }
        }
    }

    fn begin_file_export(&mut self) {
        let Some(blob) = self
            .file_tools
            .revisions
            .get(self.file_tools.selected)
            .and_then(|revision| revision.blob)
        else {
            self.notice = "This revision has no file blob to export".into();
            return;
        };
        let Some(path) = rfd::FileDialog::new()
            .set_file_name(
                Path::new(&self.file_tools.path)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("revision"),
            )
            .save_file()
        else {
            return;
        };
        match fs::symlink_metadata(&path) {
            Ok(meta) if meta.file_type().is_symlink() => {
                self.notice = "Refusing to export over a symbolic link".into()
            }
            Ok(_) => self.file_tools.pending_export = Some((path, blob)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                self.notice = self
                    .export_blob_to(&path, blob, false)
                    .unwrap_or_else(|e| e);
            }
            Err(e) => self.notice = e.to_string(),
        }
    }

    fn export_blob_to(
        &self,
        path: &Path,
        blob: git2::Oid,
        overwrite: bool,
    ) -> Result<String, String> {
        let repo = self
            .repo()
            .ok_or_else(|| "Open a repository first".to_owned())?;
        let bytes = load_blob(&repo, blob)?;
        write_export(path, &bytes, overwrite)?;
        Ok(format!("Exported revision to {}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn repo() -> (PathBuf, Repository) {
        let dir = std::env::temp_dir().join(format!(
            "grit-file-features-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        let repo = Repository::init(&dir).unwrap();
        let mut cfg = repo.config().unwrap();
        cfg.set_str("user.name", "Tester").unwrap();
        cfg.set_str("user.email", "tester@example.test").unwrap();
        (dir, repo)
    }

    fn commit_file(repo: &Repository, path: &str, data: Option<&[u8]>) -> git2::Oid {
        let mut index = repo.index().unwrap();
        if let Some(data) = data {
            let oid = repo.blob(data).unwrap();
            let entry = git2::IndexEntry {
                ctime: git2::IndexTime::new(0, 0),
                mtime: git2::IndexTime::new(0, 0),
                dev: 0,
                ino: 0,
                mode: 0o100644,
                uid: 0,
                gid: 0,
                file_size: data.len() as u32,
                id: oid,
                flags: 0,
                flags_extended: 0,
                path: path.as_bytes().to_vec(),
            };
            index.add(&entry).unwrap();
        } else {
            let _ = index.remove_path(Path::new(path));
        }
        index.write().unwrap();
        let tree_oid = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_oid).unwrap();
        let sig = repo.signature().unwrap();
        let parent = repo.head().ok().and_then(|h| h.peel_to_commit().ok());
        let parents: Vec<_> = parent.iter().collect();
        repo.commit(Some("HEAD"), &sig, &sig, "fixture", &tree, &parents)
            .unwrap()
    }

    #[test]
    fn root_update_delete_and_blob_bytes() {
        let (dir, repo) = repo();
        let binary = b"exact\0blob";
        let root = commit_file(&repo, "a.bin", Some(binary));
        let oid = repo
            .find_commit(root)
            .unwrap()
            .tree()
            .unwrap()
            .get_path(Path::new("a.bin"))
            .unwrap()
            .id();
        assert_eq!(load_blob(&repo, oid).unwrap(), binary);
        commit_file(&repo, "a.bin", Some(b"updated"));
        commit_file(&repo, "a.bin", None);
        let history = file_history(&repo, "a.bin").unwrap();
        assert_eq!(history.len(), 3);
        assert_eq!(history[0].kind, "Deleted");
        assert_eq!(history[2].kind, "Added");
        assert_eq!(
            load_blob(&repo, history[0].blob.unwrap()).unwrap(),
            b"updated"
        );
        assert_eq!(
            load_blob(&repo, history[1].blob.unwrap()).unwrap(),
            b"updated"
        );
        assert_eq!(load_blob(&repo, history[2].blob.unwrap()).unwrap(), binary);
        assert!(validate_repo_path("../outside").is_err());
        let out = dir.join("out");
        fs::write(&out, b"keep").unwrap();
        assert!(write_export(&out, b"new", false).is_err());
        assert_eq!(fs::read(&out).unwrap(), b"keep");
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            let target = dir.join("target");
            fs::write(&target, b"keep").unwrap();
            let link = dir.join("link");
            symlink(&target, &link).unwrap();
            assert!(write_export(&link, b"new", true).is_err());
            assert_eq!(fs::read(&target).unwrap(), b"keep");
        }
        let _ = fs::remove_dir_all(dir);
    }
}
