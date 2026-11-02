//! Display-only options and renderers for worktree and commit diffs.

use super::GritApp;
use git2::{Diff, DiffOptions, Repository};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffPreferences {
    pub context_lines: u32,
    pub ignore_whitespace: bool,
}

impl Default for DiffPreferences {
    fn default() -> Self {
        Self {
            context_lines: 3,
            ignore_whitespace: false,
        }
    }
}

/// Render one exact worktree path against the index (`staged == false`) or
/// the index against HEAD (`staged == true`). This only reads the repository.
pub fn render_worktree_diff(
    repo: &Repository,
    path: &str,
    staged: bool,
    prefs: &DiffPreferences,
) -> Result<String, String> {
    let mut options = configured_options(prefs);
    options
        .pathspec(path)
        .disable_pathspec_match(true)
        .include_untracked(true)
        .recurse_untracked_dirs(true)
        .show_untracked_content(true);

    let diff = if staged {
        let tree = repo.head().ok().and_then(|head| head.peel_to_tree().ok());
        repo.diff_tree_to_index(tree.as_ref(), None, Some(&mut options))
    } else {
        repo.diff_index_to_workdir(None, Some(&mut options))
    }
    .map_err(|error| error.message().to_owned())?;

    render_patch(&diff)
}

/// Render a commit against its first parent (or the empty tree for a root
/// commit), optionally limited to one exact literal path.
pub fn render_commit_diff(
    repo: &Repository,
    id: &str,
    path: Option<&str>,
    prefs: &DiffPreferences,
) -> Result<String, String> {
    let commit = repo
        .revparse_single(id)
        .and_then(|object| object.peel_to_commit())
        .map_err(|error| error.message().to_owned())?;
    let old = commit.parent(0).ok().and_then(|parent| parent.tree().ok());
    let new = commit.tree().map_err(|error| error.message().to_owned())?;
    let mut options = configured_options(prefs);
    if let Some(path) = path {
        options.pathspec(path).disable_pathspec_match(true);
    }
    let diff = repo
        .diff_tree_to_tree(old.as_ref(), Some(&new), Some(&mut options))
        .map_err(|error| error.message().to_owned())?;
    render_patch(&diff)
}

fn configured_options(prefs: &DiffPreferences) -> DiffOptions {
    let mut options = DiffOptions::new();
    options.context_lines(prefs.context_lines);
    if prefs.ignore_whitespace {
        options.ignore_whitespace(true);
    }
    options
}

fn render_patch(diff: &Diff<'_>) -> Result<String, String> {
    let mut patch = String::new();
    diff.print(git2::DiffFormat::Patch, |_, _, line| {
        if matches!(line.origin(), '+' | '-' | ' ') {
            patch.push(line.origin());
        }
        patch.push_str(std::str::from_utf8(line.content()).unwrap_or(""));
        true
    })
    .map_err(|error| error.message().to_owned())?;
    Ok(patch)
}

impl GritApp {
    pub(super) fn diff_preferences_ui(&mut self, ui: &mut eframe::egui::Ui) -> bool {
        let mut changed = false;
        ui.horizontal_wrapped(|ui| {
            changed |= ui
                .add(
                    eframe::egui::DragValue::new(&mut self.diff_preferences.context_lines)
                        .range(0..=50)
                        .prefix("Context: "),
                )
                .changed();
            changed |= ui
                .checkbox(
                    &mut self.diff_preferences.ignore_whitespace,
                    "Ignore whitespace (-w)",
                )
                .changed();
        });
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, path::PathBuf, time::SystemTime};

    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            let nonce = SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!("grit-diff-prefs-{nonce}"));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn repo(&self) -> Repository {
            Repository::init(&self.0).unwrap()
        }

        fn write(&self, path: &str, text: &str) {
            let full = self.0.join(path);
            if let Some(parent) = full.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(full, text).unwrap();
        }

        fn commit_all(repo: &Repository, message: &str) -> git2::Oid {
            let mut index = repo.index().unwrap();
            index
                .add_all(["*"], git2::IndexAddOption::DEFAULT, None)
                .unwrap();
            let tree_id = index.write_tree().unwrap();
            let tree = repo.find_tree(tree_id).unwrap();
            let signature = git2::Signature::now("Test", "test@example.com").unwrap();
            let parent = repo.head().ok().and_then(|head| head.peel_to_commit().ok());
            let parents: Vec<_> = parent.iter().collect();
            repo.commit(
                Some("HEAD"),
                &signature,
                &signature,
                message,
                &tree,
                &parents,
            )
            .unwrap()
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn whitespace_filter_keeps_content_changes_and_context_is_configurable() {
        let fixture = Fixture::new();
        let repo = fixture.repo();
        let middle = (0..15)
            .map(|line| format!("middle{line}"))
            .collect::<Vec<_>>()
            .join("\n");
        fixture.write("sample.txt", &format!("alpha  beta\n{middle}\nold\n"));
        Fixture::commit_all(&repo, "base");
        fixture.write("sample.txt", &format!("alpha beta\n{middle}\nnew\n"));

        let normal =
            render_worktree_diff(&repo, "sample.txt", false, &DiffPreferences::default()).unwrap();
        let whitespace = DiffPreferences {
            ignore_whitespace: true,
            ..DiffPreferences::default()
        };
        let ignored = render_worktree_diff(&repo, "sample.txt", false, &whitespace).unwrap();
        assert!(normal.contains("-alpha  beta") && normal.contains("+alpha beta"));
        assert!(!ignored.contains("alpha"));
        assert!(ignored.contains("-old") && ignored.contains("+new"));

        let context = |context_lines| DiffPreferences {
            context_lines,
            ignore_whitespace: false,
        };
        let zero = render_worktree_diff(&repo, "sample.txt", false, &context(0)).unwrap();
        let three = render_worktree_diff(&repo, "sample.txt", false, &context(3)).unwrap();
        let fifty = render_worktree_diff(&repo, "sample.txt", false, &context(50)).unwrap();
        assert!(zero.contains("-old") && zero.contains("+new"));
        assert!(!zero.lines().any(|line| line.starts_with(" middle")));
        assert!(three.contains(" middle0") && three.contains(" middle14"));
        assert!(fifty.lines().count() > three.lines().count());

        let mut index = repo.index().unwrap();
        index.add_path(std::path::Path::new("sample.txt")).unwrap();
        index.write().unwrap();
        let before = repo
            .status_file(std::path::Path::new("sample.txt"))
            .unwrap();
        let staged = render_worktree_diff(&repo, "sample.txt", true, &whitespace).unwrap();
        assert!(!staged.contains("alpha"));
        assert!(staged.contains("-old") && staged.contains("+new"));
        assert_eq!(
            repo.status_file(std::path::Path::new("sample.txt"))
                .unwrap(),
            before
        );
        let mixed_commit = Fixture::commit_all(&repo, "mixed changes");
        let commit_patch = render_commit_diff(
            &repo,
            &mixed_commit.to_string(),
            Some("sample.txt"),
            &whitespace,
        )
        .unwrap();
        assert!(!commit_patch.contains("alpha"));
        assert!(commit_patch.contains("-old") && commit_patch.contains("+new"));

        fixture.write("sample.txt", &format!("alpha    beta\n{middle}\nnew\n"));
        let mut index = repo.index().unwrap();
        index.add_path(std::path::Path::new("sample.txt")).unwrap();
        index.write().unwrap();
        let staged_ws = render_worktree_diff(&repo, "sample.txt", true, &whitespace).unwrap();
        assert!(staged_ws.is_empty());
        let git_staged = std::process::Command::new("git")
            .args(["diff", "--cached", "--ignore-all-space", "--", "sample.txt"])
            .current_dir(&fixture.0)
            .output()
            .unwrap();
        assert!(git_staged.status.success());
        assert!(git_staged.stdout.is_empty());
        let whitespace_commit = Fixture::commit_all(&repo, "whitespace only");
        assert!(
            render_commit_diff(
                &repo,
                &whitespace_commit.to_string(),
                Some("sample.txt"),
                &whitespace
            )
            .unwrap()
            .is_empty()
        );
    }

    #[test]
    fn root_unborn_untracked_and_literal_paths_render() {
        let fixture = Fixture::new();
        let repo = fixture.repo();
        fixture.write("a*.txt", "literal\n");
        fixture.write("abc.txt", "glob match\n");
        let unborn =
            render_worktree_diff(&repo, "a*.txt", false, &DiffPreferences::default()).unwrap();
        assert!(unborn.contains("literal"));
        assert!(!unborn.contains("glob match"));
        assert!(
            render_worktree_diff(&repo, "a*.txt", true, &DiffPreferences::default())
                .unwrap()
                .is_empty()
        );
        let mut index = repo.index().unwrap();
        index.add_path(std::path::Path::new("a*.txt")).unwrap();
        index.write().unwrap();
        let staged =
            render_worktree_diff(&repo, "a*.txt", true, &DiffPreferences::default()).unwrap();
        assert!(staged.contains("literal"));
        assert!(!staged.contains("glob match"));

        Fixture::commit_all(&repo, "root");
        let head = repo.head().unwrap().target().unwrap().to_string();
        let root = render_commit_diff(&repo, &head, None, &DiffPreferences::default()).unwrap();
        assert!(root.contains("literal") && root.contains("glob match"));
        let selected =
            render_commit_diff(&repo, &head, Some("a*.txt"), &DiffPreferences::default()).unwrap();
        assert!(selected.contains("literal"));
        assert!(!selected.contains("glob match"));
    }
}
