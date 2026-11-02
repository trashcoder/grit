use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_REPO: AtomicU64 = AtomicU64::new(0);

struct TestRepo(PathBuf);

impl TestRepo {
    fn new() -> Self {
        let n = NEXT_REPO.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "grit-regression-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos(),
            n
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn open(&self) -> Repository {
        Repository::init(&self.0).unwrap()
    }
}

impl Drop for TestRepo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn commit_file(repo: &Repository, path: &str, contents: &str, message: &str) -> git2::Oid {
    std::fs::write(repo.workdir().unwrap().join(path), contents).unwrap();
    let mut index = repo.index().unwrap();
    index.add_path(Path::new(path)).unwrap();
    index.write().unwrap();
    let tree_id = index.write_tree().unwrap();
    let tree = repo.find_tree(tree_id).unwrap();
    let signature = git2::Signature::now("Grit Test", "grit-test@example.invalid").unwrap();
    let parent = repo.head().ok().and_then(|head| head.peel_to_commit().ok());
    let parents: Vec<&git2::Commit<'_>> = parent.iter().collect();
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

#[test]
fn diff_file_side_keeps_staged_and_unstaged_edits_separate() {
    let dir = TestRepo::new();
    let repo = dir.open();
    commit_file(&repo, "notes.txt", "base\n", "base");

    std::fs::write(dir.0.join("notes.txt"), "staged version\n").unwrap();
    let mut index = repo.index().unwrap();
    index.add_path(Path::new("notes.txt")).unwrap();
    index.write().unwrap();
    std::fs::write(dir.0.join("notes.txt"), "worktree version\n").unwrap();

    let staged = diff_file_side(&repo, "notes.txt", true);
    let unstaged = diff_file_side(&repo, "notes.txt", false);
    assert!(
        staged.contains("staged version"),
        "staged patch missing: {staged}"
    );
    assert!(
        !staged.contains("worktree version"),
        "staged patch leaked worktree content: {staged}"
    );
    assert!(
        unstaged.contains("worktree version"),
        "unstaged patch missing: {unstaged}"
    );
    assert!(
        !unstaged.contains("base"),
        "unstaged patch leaked committed base content: {unstaged}"
    );
    assert!(
        staged
            .lines()
            .any(|line| line.starts_with('+') && line.contains("staged version")),
        "staged addition marker missing: {staged}"
    );
    assert!(
        unstaged
            .lines()
            .any(|line| line.starts_with('-') && line.contains("staged version")),
        "unstaged patch does not use the staged version as its base: {unstaged}"
    );
    assert!(
        unstaged
            .lines()
            .any(|line| line.starts_with('+') && line.contains("worktree version")),
        "unstaged addition marker missing: {unstaged}"
    );
}

#[test]
fn staged_diff_works_before_first_commit() {
    let dir = TestRepo::new();
    let repo = dir.open();
    std::fs::write(dir.0.join("initial.txt"), "first staged content\n").unwrap();
    let mut index = repo.index().unwrap();
    index.add_path(Path::new("initial.txt")).unwrap();
    index.write().unwrap();

    let staged = diff_file_side(&repo, "initial.txt", true);
    assert!(
        staged.contains("first staged content"),
        "initial staged patch missing: {staged}"
    );
    assert!(
        staged
            .lines()
            .any(|line| line.starts_with('+') && line.contains("first staged content")),
        "initial staged addition marker missing: {staged}"
    );
}

#[test]
fn unstaged_diff_previews_untracked_file() {
    let dir = TestRepo::new();
    let repo = dir.open();
    std::fs::write(dir.0.join("new-file.txt"), "untracked preview content\n").unwrap();

    let unstaged = diff_file_side(&repo, "new-file.txt", false);
    assert!(
        unstaged.contains("untracked preview content"),
        "untracked preview missing: {unstaged}"
    );
    assert!(
        unstaged
            .lines()
            .any(|line| line.starts_with('+') && line.contains("untracked preview content")),
        "untracked addition marker missing: {unstaged}"
    );
}

#[test]
fn statuses_reports_staged_and_unstaged_rows_for_same_path() {
    let dir = TestRepo::new();
    let repo = dir.open();
    commit_file(&repo, "notes.txt", "base\n", "base");

    std::fs::write(dir.0.join("notes.txt"), "staged version\n").unwrap();
    let mut index = repo.index().unwrap();
    index.add_path(Path::new("notes.txt")).unwrap();
    index.write().unwrap();
    std::fs::write(dir.0.join("notes.txt"), "worktree version\n").unwrap();

    let rows: Vec<_> = statuses(&repo)
        .into_iter()
        .filter(|row| row.path == "notes.txt")
        .collect();
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().any(|row| row.staged));
    assert!(rows.iter().any(|row| !row.staged));
}

#[test]
fn history_records_parent_ids_and_reachable_branch_refs() {
    let dir = TestRepo::new();
    let repo = dir.open();
    let first = commit_file(&repo, "first.txt", "first\n", "first commit");
    let second = commit_file(&repo, "second.txt", "second\n", "second commit");

    let commits = history(&repo).unwrap();
    let second_view = commits
        .iter()
        .find(|commit| commit.id == second.to_string())
        .unwrap();
    assert_eq!(second_view.parents, vec![first.to_string()]);
    assert!(
        second_view
            .refs
            .iter()
            .any(|reference| reference == "master" || reference == "main")
    );
    assert!(commits.iter().any(|commit| commit.id == first.to_string()));
}

#[test]
fn graph_layout_keeps_two_merge_parent_lanes_connected() {
    let make_commit = |id: &str, parents: &[&str]| Commit {
        id: id.to_owned(),
        parents: parents.iter().map(|parent| (*parent).to_owned()).collect(),
        short: id.to_owned(),
        message: id.to_owned(),
        author: "Grit Test".into(),
        date: String::new(),
        refs: Vec::new(),
        ..Default::default()
    };
    let commits = vec![
        make_commit("merge", &["left", "right"]),
        make_commit("left", &["base"]),
        make_commit("right", &["base"]),
        make_commit("base", &[]),
    ];

    let rows = graph_layout(&commits);
    assert_eq!(rows[0].parents, vec![0, 1]);
    assert_eq!(rows[1].lane, 0);
    assert_eq!(rows[2].lane, 1);
    assert_eq!(rows[3].lane, 0);
}

#[test]
fn refresh_preserves_selected_staged_side_for_same_path() {
    let dir = TestRepo::new();
    let repo = dir.open();
    commit_file(&repo, "notes.txt", "committed base\n", "base");
    std::fs::write(dir.0.join("notes.txt"), "index version\n").unwrap();
    let mut index = repo.index().unwrap();
    index.add_path(Path::new("notes.txt")).unwrap();
    index.write().unwrap();
    std::fs::write(dir.0.join("notes.txt"), "worktree version\n").unwrap();

    let mut app = GritApp::default();
    app.open(dir.0.clone());
    app.selected_staged = true;
    app.select_file("notes.txt".to_owned());
    app.refresh();

    assert_eq!(app.selected_file.as_deref(), Some("notes.txt"));
    assert!(app.selected_staged);
    assert!(
        app.diff.contains("index version"),
        "staged diff missing: {}",
        app.diff
    );
    assert!(
        !app.diff.contains("worktree version"),
        "staged diff leaked worktree content: {}",
        app.diff
    );
}

#[test]
fn history_scope_contains_only_branch_ancestors_and_can_be_cleared() {
    let dir = TestRepo::new();
    let repo = dir.open();
    let base = commit_file(&repo, "base.txt", "base\n", "base");
    repo.branch("feature", &repo.find_commit(base).unwrap(), false)
        .unwrap();
    let main_only = commit_file(&repo, "main-only.txt", "main\n", "main only");

    repo.set_head("refs/heads/feature").unwrap();
    repo.checkout_head(Some(git2::build::CheckoutBuilder::new().safe()))
        .unwrap();
    let feature = commit_file(&repo, "feature-only.txt", "feature\n", "feature only");

    let mut app = GritApp {
        path: Some(dir.0.clone()),
        ..Default::default()
    };
    app.set_history_scope(Some("refs/heads/feature".to_owned()));
    let (scope_name, ids) = app.history_scope.as_ref().unwrap();
    assert_eq!(scope_name, "refs/heads/feature");
    assert_eq!(ids, &HashSet::from([base.to_string(), feature.to_string()]));
    assert!(!ids.contains(&main_only.to_string()));

    app.set_history_scope(None);
    assert!(app.history_scope.is_none());
}

#[test]
fn literal_filename_diff_and_editable_patch_keep_origins() {
    let dir = TestRepo::new();
    let repo = dir.open();
    commit_file(&repo, "a[b].txt", "old\n", "base");
    commit_file(&repo, "ab.txt", "other\n", "other");
    std::fs::write(dir.0.join("a[b].txt"), "++ body\n").unwrap();
    std::fs::write(dir.0.join("ab.txt"), "unrelated\n").unwrap();
    let patch = unstaged_patch(&repo, "a[b].txt").unwrap();
    assert!(patch.contains("-old\n"));
    assert!(patch.contains("+++ body\n"));
    assert!(!patch.contains("unrelated"));
    assert!(patch_has_changes(&patch));
    assert!(!patch_has_changes("diff --git a/x b/x\n--- a/x\n+++ b/x\n"));
    assert!(!diff_file_side(&repo, "a[b].txt", false).contains("unrelated"));
}

#[test]
fn display_diff_preferences_keep_raw_patch_and_stage_selected_zero_context() {
    let dir = TestRepo::new();
    let repo = dir.open();
    commit_file(&repo, "notes.txt", "one\ntwo\nthree\nfour\n", "base");
    std::fs::write(dir.0.join("notes.txt"), "one\nTWO\nthree\nFOUR\n").unwrap();
    let mut app = GritApp::default();
    app.open(dir.0.clone());
    app.tab = 0;
    app.selected_staged = false;
    app.select_file("notes.txt".into());
    let raw = app.diff.clone();
    app.diff_preferences.context_lines = 0;
    app.ensure_display_diff();
    assert_eq!(app.diff, raw);
    assert_ne!(app.display_diff, raw);
    let selected = app
        .display_diff
        .lines()
        .position(|line| line == "+TWO")
        .unwrap();
    app.selected_diff_lines.insert(selected);
    app.apply_selected_lines();
    let (_, receiver) = app.operation.take().expect("Index operation started");
    receiver
        .recv_timeout(std::time::Duration::from_secs(5))
        .unwrap()
        .unwrap();
    let mut index = repo.index().unwrap();
    index.read(true).unwrap();
    let entry = index.get_path(Path::new("notes.txt"), 0).unwrap();
    assert_eq!(
        repo.find_blob(entry.id).unwrap().content(),
        b"one\ntwo\nTWO\nthree\nfour\n"
    );
    assert_eq!(
        std::fs::read(dir.0.join("notes.txt")).unwrap(),
        b"one\nTWO\nthree\nFOUR\n"
    );
}

#[test]
fn whitespace_display_is_read_only_and_disables_line_staging() {
    let dir = TestRepo::new();
    let repo = dir.open();
    commit_file(&repo, "notes.txt", "alpha  beta\n", "base");
    std::fs::write(dir.0.join("notes.txt"), "alpha beta\n").unwrap();
    let mut app = GritApp::default();
    app.open(dir.0.clone());
    app.tab = 0;
    app.select_file("notes.txt".into());
    let raw = app.diff.clone();
    app.diff_preferences.ignore_whitespace = true;
    app.ensure_display_diff();
    assert_eq!(app.diff, raw);
    assert!(!app.display_diff.contains("+alpha beta"));
    app.selected_diff_lines.insert(7);
    app.apply_selected_lines();
    assert!(app.operation.is_none());
    assert!(app.notice.contains("Show whitespace"));
    assert!(statuses(&repo).iter().all(|c| !c.staged));
}
