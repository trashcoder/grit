use super::*;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::{
    ffi::OsString,
    fs::{self, OpenOptions},
    io::Write,
    path::{Component, Path},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

const MAX_TOOL_ARG_LINES: usize = 64;
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Default)]
pub(super) struct ExternalDiffTools {
    /// Executable path and one argv template per line. These fields are public to the root
    /// persistence layer; templates are data and are never parsed by a shell.
    pub program: String,
    pub diff_args: String,
    pub merge_args: String,
}

#[derive(Clone, Copy)]
enum DiffSource {
    WorkingOrStaged,
    Historical,
}

struct TempInputs {
    dir: PathBuf,
    old: PathBuf,
    new: PathBuf,
}

impl Drop for TempInputs {
    fn drop(&mut self) {
        cleanup_temp_dir(&self.dir);
    }
}

struct TempMerge {
    dir: PathBuf,
    base: PathBuf,
    ours: PathBuf,
    theirs: PathBuf,
    merged: PathBuf,
}

impl Drop for TempMerge {
    fn drop(&mut self) {
        cleanup_temp_dir(&self.dir);
    }
}

impl GritApp {
    pub(super) fn external_diff_tools_ui(&mut self, ui: &mut egui::Ui) {
        let busy = self.operation.is_some();
        ui.separator();
        ui.heading("External diff and merge tools");
        ui.label("Executable path");
        ui.add(
            egui::TextEdit::singleline(&mut self.external_diff.program)
                .hint_text("/path/to/tool (launched directly, without a shell)"),
        );
        ui.label("Diff argv templates (one argument per line; {old}, {new})");
        ui.add(egui::TextEdit::multiline(&mut self.external_diff.diff_args).desired_rows(2));
        ui.label(
            "Merge argv templates (one argument per line; {base}, {ours}, {theirs}, {merged})",
        );
        ui.add(egui::TextEdit::multiline(&mut self.external_diff.merge_args).desired_rows(3));

        let configured = !self.external_diff.program.trim().is_empty();
        let selected = self.selected_file.clone();
        let has_historical = self.selected_commit.is_some() && self.selected_commit_file.is_some();
        let has_working = self.selected_commit.is_none() && selected.is_some();
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    !busy && configured && (has_historical || has_working),
                    egui::Button::new("External diff selected file"),
                )
                .clicked()
            {
                let (source, commit, path, staged) = if has_historical {
                    (
                        DiffSource::Historical,
                        self.selected_commit.clone(),
                        self.selected_commit_file.clone(),
                        false,
                    )
                } else {
                    (
                        DiffSource::WorkingOrStaged,
                        None,
                        selected.clone(),
                        self.selected_staged,
                    )
                };
                if let Some(path) = path {
                    let args = self.external_diff.diff_args.clone();
                    let program = self.external_diff.program.clone();
                    self.start_external_tool("External diff", move |root| {
                        run_diff_tool(
                            &root,
                            &program,
                            &args,
                            source,
                            commit.as_deref(),
                            &path,
                            staged,
                        )
                    });
                }
            }
            if ui
                .add_enabled(
                    !busy
                        && configured
                        && has_working
                        && !self.external_diff.merge_args.trim().is_empty(),
                    egui::Button::new("External merge selected conflict"),
                )
                .clicked()
                && let Some(path) = selected
            {
                let args = self.external_diff.merge_args.clone();
                let program = self.external_diff.program.clone();
                self.start_external_tool("External merge", move |root| {
                    run_merge_tool(&root, &program, &args, &path)
                });
            }
        });
        if self.selected_commit.is_some() {
            ui.label("Diff uses the selected historical file against its first parent.");
        } else {
            ui.label(if self.selected_staged {
                "Diff compares HEAD with the index."
            } else {
                "Diff compares the index with the working tree."
            });
            ui.label("Merge is available only for the selected file with unresolved index stages. The tool edits the working file; grit does not stage its result.");
        }
    }

    fn start_external_tool<F>(&mut self, label: &'static str, work: F)
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
            let result = work(root);
            let _ = tx.send(result);
        });
        self.notice = format!("{label} running…");
        self.operation = Some((label.into(), rx));
    }
}

fn parse_arg_templates(raw: &str, allowed: &[&str]) -> Result<Vec<String>, String> {
    let lines = raw.lines().collect::<Vec<_>>();
    if lines.len() > MAX_TOOL_ARG_LINES {
        return Err(format!(
            "Use at most {MAX_TOOL_ARG_LINES} argv template lines"
        ));
    }
    let mut args = Vec::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let mut rest = line;
        while let Some(open) = rest.find('{') {
            let after = &rest[open + 1..];
            let close = after
                .find('}')
                .ok_or("Unclosed placeholder in argv template")?;
            let placeholder = &after[..close];
            if !allowed.contains(&placeholder) {
                return Err(format!("Unknown placeholder {{{placeholder}}}"));
            }
            rest = &after[close + 1..];
        }
        if rest.contains('}') {
            return Err("Unmatched closing brace in argv template".into());
        }
        args.push(line.to_owned());
    }
    Ok(args)
}

fn expand_args(templates: &[String], values: &[(&str, &Path)]) -> Vec<OsString> {
    templates
        .iter()
        .map(|template| {
            let mut arg = OsString::new();
            let mut rest = template.as_str();
            while let Some(open) = rest.find('{') {
                arg.push(&rest[..open]);
                let after = &rest[open + 1..];
                let close = after.find('}').expect("templates are validated");
                let key = &after[..close];
                if let Some((_, path)) = values.iter().find(|(name, _)| *name == key) {
                    arg.push(path.as_os_str());
                }
                rest = &after[close + 1..];
            }
            arg.push(rest);
            arg
        })
        .collect()
}

fn safe_relative(path: &str) -> Result<&Path, String> {
    let p = Path::new(path);
    if path.is_empty()
        || p.is_absolute()
        || p.components().any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err("Selected path is not a safe repository-relative file".into());
    }
    Ok(p)
}

fn confined_worktree_path(
    root: &Path,
    relative: &str,
    require_existing: bool,
) -> Result<PathBuf, String> {
    let rel = safe_relative(relative)?;
    let canonical_root = root
        .canonicalize()
        .map_err(|e| format!("Could not resolve repository: {e}"))?;
    let mut parent = canonical_root.clone();
    if let Some(parent_rel) = rel.parent() {
        for component in parent_rel.components() {
            parent.push(component.as_os_str());
            let metadata = fs::symlink_metadata(&parent)
                .map_err(|e| format!("Could not resolve selected file directory: {e}"))?;
            if metadata.file_type().is_symlink() {
                return Err("Refusing to traverse a symlink in an external tool path".into());
            }
            if !metadata.is_dir() {
                return Err("Selected worktree parent is not a directory".into());
            }
            let canonical = parent.canonicalize().map_err(|e| e.to_string())?;
            if !canonical.starts_with(&canonical_root) {
                return Err("Selected worktree path escapes the repository".into());
            }
            parent = canonical;
        }
    }
    let candidate = canonical_root.join(rel);
    match fs::symlink_metadata(&candidate) {
        Ok(meta) if meta.file_type().is_symlink() => {
            Err("Refusing to read or write a symlink as an external tool input/output".into())
        }
        Ok(meta) if !meta.is_file() => Err("Selected worktree path is not a regular file".into()),
        Ok(_) => {
            let canonical = candidate.canonicalize().map_err(|e| e.to_string())?;
            if !canonical.starts_with(&canonical_root) {
                return Err("Selected worktree path escapes the repository".into());
            }
            Ok(canonical)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && !require_existing => {
            Ok(parent.join(
                candidate
                    .file_name()
                    .ok_or("Selected worktree path has no name")?,
            ))
        }
        Err(error) => Err(format!("Could not resolve selected worktree file: {error}")),
    }
}

fn blob_at(
    repo: &Repository,
    tree: Option<&git2::Tree<'_>>,
    relative: &str,
) -> Result<Option<Vec<u8>>, String> {
    let Some(tree) = tree else { return Ok(None) };
    let path = safe_relative(relative)?;
    match tree.get_path(path) {
        Ok(entry) => {
            if entry.kind() != Some(git2::ObjectType::Blob) {
                return Err("External tools support regular blob files only".into());
            }
            Ok(Some(
                repo.find_blob(entry.id())
                    .map_err(|e| e.to_string())?
                    .content()
                    .to_vec(),
            ))
        }
        Err(error) if error.code() == git2::ErrorCode::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

fn index_blob(repo: &Repository, relative: &str, stage: i32) -> Result<Option<Vec<u8>>, String> {
    let path = safe_relative(relative)?;
    let index = repo.index().map_err(|e| e.to_string())?;
    let Some(entry) = index.get_path(path, stage) else {
        return Ok(None);
    };
    Ok(Some(
        repo.find_blob(entry.id)
            .map_err(|e| e.to_string())?
            .content()
            .to_vec(),
    ))
}

fn make_temp_dir() -> Result<PathBuf, String> {
    let base = std::env::temp_dir()
        .canonicalize()
        .map_err(|e| format!("Could not resolve temporary directory: {e}"))?;
    for _ in 0..100 {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let dir = base.join(format!(
            "grit-external-{}-{nonce}-{}",
            std::process::id(),
            TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        match fs::create_dir(&dir) {
            Ok(()) => return Ok(dir),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.to_string()),
        }
    }
    Err("Could not create a unique temporary directory".into())
}

// Windows must clear the read-only attribute on temporary diff inputs before deletion.
#[cfg_attr(not(unix), allow(clippy::permissions_set_readonly_false))]
fn cleanup_temp_dir(path: &Path) {
    let Ok(entries) = fs::read_dir(path) else {
        let _ = fs::remove_dir_all(path);
        return;
    };
    for entry in entries.flatten() {
        let item = entry.path();
        if let Ok(meta) = fs::symlink_metadata(&item) {
            if meta.is_dir() && !meta.file_type().is_symlink() {
                cleanup_temp_dir(&item);
            } else if meta.is_file() && meta.permissions().readonly() {
                let mut permissions = meta.permissions();
                #[cfg(unix)]
                permissions.set_mode(permissions.mode() | 0o200);
                #[cfg(not(unix))]
                permissions.set_readonly(false);
                let _ = fs::set_permissions(&item, permissions);
            }
        }
    }
    let _ = fs::remove_dir_all(path);
}

fn write_input(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| e.to_string())?;
    file.write_all(bytes).map_err(|e| e.to_string())?;
    let mut permissions = file.metadata().map_err(|e| e.to_string())?.permissions();
    permissions.set_readonly(true);
    fs::set_permissions(path, permissions).map_err(|e| e.to_string())
}

fn materialize_pair(old: Option<&[u8]>, new: Option<&[u8]>) -> Result<TempInputs, String> {
    let dir = make_temp_dir()?;
    let old_path = dir.join("old input");
    let new_path = dir.join("new input");
    if let Err(error) = write_input(&old_path, old.unwrap_or_default())
        .and_then(|_| write_input(&new_path, new.unwrap_or_default()))
    {
        cleanup_temp_dir(&dir);
        return Err(error);
    }
    Ok(TempInputs {
        dir,
        old: old_path,
        new: new_path,
    })
}

fn run_command(program: &str, args: Vec<OsString>, cwd: &Path) -> Result<String, String> {
    if program.trim().is_empty() {
        return Err("Configure an external tool executable path".into());
    }
    let output = Command::new(program)
        .args(args)
        .current_dir(cwd)
        .output()
        .map_err(|e| format!("Could not start external tool: {e}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_owned())
    }
}

fn run_diff_tool(
    root: &Path,
    program: &str,
    raw_args: &str,
    source: DiffSource,
    commit_id: Option<&str>,
    relative: &str,
    staged: bool,
) -> Result<String, String> {
    let templates = parse_arg_templates(raw_args, &["old", "new"])?;
    let repo = Repository::open(root).map_err(|e| e.to_string())?;
    let (old, new) = match source {
        DiffSource::Historical => {
            let commit = repo
                .revparse_single(commit_id.ok_or("No historical commit selected")?)
                .and_then(|object| object.peel_to_commit())
                .map_err(|e| e.to_string())?;
            let parent = commit.parent(0).ok().and_then(|parent| parent.tree().ok());
            let tree = commit.tree().map_err(|e| e.to_string())?;
            (
                blob_at(&repo, parent.as_ref(), relative)?,
                blob_at(&repo, Some(&tree), relative)?,
            )
        }
        DiffSource::WorkingOrStaged if staged => {
            let head_tree = repo.head().ok().and_then(|head| head.peel_to_tree().ok());
            (
                blob_at(&repo, head_tree.as_ref(), relative)?,
                index_blob(&repo, relative, 0)?,
            )
        }
        DiffSource::WorkingOrStaged => {
            let index = index_blob(&repo, relative, 0)?;
            let path = confined_worktree_path(root, relative, false)?;
            let worktree = match fs::read(path) {
                Ok(bytes) => Some(bytes),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => return Err(error.to_string()),
            };
            (index, worktree)
        }
    };
    let inputs = materialize_pair(old.as_deref(), new.as_deref())?;
    let args = expand_args(&templates, &[("old", &inputs.old), ("new", &inputs.new)]);
    let result = run_command(program, args, root);
    drop(inputs);
    result
}

struct UnmergedBlobs {
    base: Option<Vec<u8>>,
    ours: Option<Vec<u8>>,
    theirs: Option<Vec<u8>>,
}

fn unmerged_stages(repo: &Repository, relative: &str) -> Result<UnmergedBlobs, String> {
    let path = safe_relative(relative)?;
    let index = repo.index().map_err(|e| e.to_string())?;
    let mut found = false;
    for conflict in index.conflicts().map_err(|e| e.to_string())? {
        let conflict = conflict.map_err(|e| e.to_string())?;
        let matches = [&conflict.ancestor, &conflict.our, &conflict.their]
            .into_iter()
            .flatten()
            .any(|entry| Path::new(std::str::from_utf8(&entry.path).unwrap_or("\0")) == path);
        if matches {
            found = true;
            break;
        }
    }
    if !found {
        return Err("Selected file is not an unresolved index conflict".into());
    }
    let base = index_blob(repo, relative, 1)?;
    let ours = index_blob(repo, relative, 2)?;
    let theirs = index_blob(repo, relative, 3)?;
    Ok(UnmergedBlobs { base, ours, theirs })
}

fn run_merge_tool(
    root: &Path,
    program: &str,
    raw_args: &str,
    relative: &str,
) -> Result<String, String> {
    let templates = parse_arg_templates(raw_args, &["base", "ours", "theirs", "merged"])?;
    let repo = Repository::open(root).map_err(|e| e.to_string())?;
    let blobs = unmerged_stages(&repo, relative)?;
    let merged = confined_worktree_path(root, relative, false)?;
    let dir = make_temp_dir()?;
    let base_path = dir.join("base input");
    let ours_path = dir.join("ours input");
    let theirs_path = dir.join("theirs input");
    let write_result = write_input(&base_path, blobs.base.as_deref().unwrap_or_default())
        .and_then(|_| write_input(&ours_path, blobs.ours.as_deref().unwrap_or_default()))
        .and_then(|_| write_input(&theirs_path, blobs.theirs.as_deref().unwrap_or_default()));
    if let Err(error) = write_result {
        cleanup_temp_dir(&dir);
        return Err(error);
    }
    let inputs = TempMerge {
        dir,
        base: base_path,
        ours: ours_path,
        theirs: theirs_path,
        merged,
    };
    let args = expand_args(
        &templates,
        &[
            ("base", &inputs.base),
            ("ours", &inputs.ours),
            ("theirs", &inputs.theirs),
            ("merged", &inputs.merged),
        ],
    );
    let result = run_command(program, args, root);
    drop(inputs);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    struct TestRepo(PathBuf);
    impl TestRepo {
        fn new(name: &str) -> Self {
            let p = std::env::temp_dir().join(format!(
                "grit-{name}-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir_all(&p).unwrap();
            assert!(
                Command::new("git")
                    .args(["init", "-q", p.to_str().unwrap()])
                    .status()
                    .unwrap()
                    .success()
            );
            let run = |args: &[&str]| {
                Command::new("git")
                    .arg("-C")
                    .arg(&p)
                    .args(args)
                    .output()
                    .unwrap()
            };
            assert!(run(&["config", "user.name", "Test User"]).status.success());
            assert!(
                run(&["config", "user.email", "test@example.test"])
                    .status
                    .success()
            );
            Self(p)
        }
        fn run(&self, args: &[&str]) -> std::process::Output {
            Command::new("git")
                .arg("-C")
                .arg(&self.0)
                .args(args)
                .output()
                .unwrap()
        }
    }
    impl Drop for TestRepo {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[cfg(unix)]
    fn script(dir: &Path, body: &str) -> PathBuf {
        let path = dir.join("capture tool.sh");
        fs::write(&path, format!("#!/bin/sh\nset -eu\n{body}\n")).unwrap();
        let mut permissions = fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&path, permissions).unwrap();
        path
    }

    #[test]
    #[cfg(unix)]
    fn external_diff_passes_exact_absolute_paths_and_contents_as_argv() {
        let repo = TestRepo::new("external diff");
        let file = "file with spaces.txt";
        fs::write(repo.0.join(file), "before\n").unwrap();
        assert!(repo.run(&["add", "--", file]).status.success());
        assert!(repo.run(&["commit", "-qm", "base"]).status.success());
        fs::write(repo.0.join(file), "staged\n").unwrap();
        assert!(repo.run(&["add", "--", file]).status.success());
        fs::write(repo.0.join(file), "after\n").unwrap();
        let capture = repo.0.join("captured args");
        let tool = script(
            &repo.0,
            "printf '%s\\n' \"$#\" \"$1\" \"$2\" > \"$3\"; cat \"$1\" >> \"$3\"; cat \"$2\" >> \"$3\"",
        );
        let args = format!("{{old}}\n{{new}}\n{}", capture.display());
        run_diff_tool(
            &repo.0,
            tool.to_str().unwrap(),
            &args,
            DiffSource::WorkingOrStaged,
            None,
            file,
            true,
        )
        .unwrap();
        let result = fs::read_to_string(&capture).unwrap();
        let lines = result.lines().collect::<Vec<_>>();
        assert_eq!(lines[0], "3");
        assert!(Path::new(lines[1]).is_absolute());
        assert!(Path::new(lines[2]).is_absolute());
        assert!(lines[1].contains("grit-external-"));
        assert!(lines[2].contains("grit-external-"));
        assert_eq!(&lines[3..], &["before", "staged"]);
        assert!(!Path::new(lines[1]).exists());
        assert!(!Path::new(lines[2]).exists());

        run_diff_tool(
            &repo.0,
            tool.to_str().unwrap(),
            &args,
            DiffSource::WorkingOrStaged,
            None,
            file,
            false,
        )
        .unwrap();
        let result = fs::read_to_string(&capture).unwrap();
        let lines = result.lines().collect::<Vec<_>>();
        assert_eq!(&lines[3..], &["staged", "after"]);

        assert!(
            repo.run(&["commit", "-qm", "staged snapshot"])
                .status
                .success()
        );
        let head = String::from_utf8(repo.run(&["rev-parse", "HEAD"]).stdout).unwrap();
        run_diff_tool(
            &repo.0,
            tool.to_str().unwrap(),
            &args,
            DiffSource::Historical,
            Some(head.trim()),
            file,
            false,
        )
        .unwrap();
        let result = fs::read_to_string(&capture).unwrap();
        let lines = result.lines().collect::<Vec<_>>();
        assert_eq!(&lines[3..], &["before", "staged"]);
    }

    #[test]
    #[cfg(unix)]
    fn external_merge_reads_conflict_stages_writes_worktree_and_never_stages_result() {
        let repo = TestRepo::new("external merge");
        fs::write(repo.0.join("conflict.txt"), "base\n").unwrap();
        assert!(repo.run(&["add", "conflict.txt"]).status.success());
        assert!(repo.run(&["commit", "-qm", "base"]).status.success());
        assert!(repo.run(&["checkout", "-qb", "other"]).status.success());
        fs::write(repo.0.join("conflict.txt"), "theirs\n").unwrap();
        assert!(repo.run(&["commit", "-qam", "theirs"]).status.success());
        assert!(
            repo.run(&["checkout", "-q", "master"]).status.success()
                || repo.run(&["checkout", "-q", "main"]).status.success()
        );
        fs::write(repo.0.join("conflict.txt"), "ours\n").unwrap();
        assert!(repo.run(&["commit", "-qam", "ours"]).status.success());
        let merge = repo.run(&["merge", "other"]);
        assert!(!merge.status.success());
        let capture = repo.0.join("merged arg");
        let tool = script(
            &repo.0,
            "printf '%s\\n' \"$#\" \"$1\" \"$2\" \"$3\" \"$4\" > \"$5\"; printf 'BASE\\n' >> \"$5\"; cat \"$1\" >> \"$5\"; printf 'OURS\\n' >> \"$5\"; cat \"$2\" >> \"$5\"; printf 'THEIRS\\n' >> \"$5\"; cat \"$3\" >> \"$5\"; printf 'resolved\\n' > \"$4\"",
        );
        let args = format!(
            "{{base}}\n{{ours}}\n{{theirs}}\n{{merged}}\n{}",
            capture.display()
        );
        run_merge_tool(&repo.0, tool.to_str().unwrap(), &args, "conflict.txt").unwrap();
        let values = fs::read_to_string(&capture).unwrap();
        let fields = values.lines().collect::<Vec<_>>();
        assert_eq!(fields[0], "5");
        assert!(
            fields[1..4]
                .iter()
                .all(|field| Path::new(field).is_absolute())
        );
        assert_eq!(
            &fields[5..],
            &["BASE", "base", "OURS", "ours", "THEIRS", "theirs"]
        );
        assert_eq!(
            fs::read_to_string(repo.0.join("conflict.txt")).unwrap(),
            "resolved\n"
        );
        assert!(
            !repo.run(&["ls-files", "-u"]).stdout.is_empty(),
            "merge result must not be staged"
        );
        for input in &fields[1..4] {
            assert!(
                !Path::new(input).exists(),
                "temporary merge inputs must be cleaned up"
            );
        }
    }

    #[test]
    fn rejects_unknown_placeholders_and_path_escape() {
        assert!(parse_arg_templates("{old}\n{oops}", &["old", "new"]).is_err());
        assert!(safe_relative("../outside").is_err());
    }

    #[test]
    #[cfg(unix)]
    fn refuses_symlinked_worktree_inputs_and_merge_targets() {
        use std::os::unix::fs::symlink;
        let repo = TestRepo::new("external symlink");
        fs::write(repo.0.join("real.txt"), "contents\n").unwrap();
        symlink("real.txt", repo.0.join("linked.txt")).unwrap();
        assert!(confined_worktree_path(&repo.0, "linked.txt", true).is_err());
    }
}
