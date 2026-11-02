//! Line-level patch selection helpers for staging and unstaging.
use std::collections::HashSet;

#[derive(Clone)]
struct Line {
    text: String,
    eol: &'static str,
}
fn split_lines(s: &str) -> Vec<Line> {
    let mut out = Vec::new();
    let mut start = 0;
    for (i, b) in s.bytes().enumerate() {
        if b == b'\n' {
            let end = if i > start && s.as_bytes()[i - 1] == b'\r' {
                i - 1
            } else {
                i
            };
            out.push(Line {
                text: s[start..end].to_owned(),
                eol: if end < i { "\r\n" } else { "\n" },
            });
            start = i + 1;
        }
    }
    if start < s.len() {
        out.push(Line {
            text: s[start..].to_owned(),
            eol: "",
        });
    }
    out
}
fn hunk_header(s: &str) -> Option<(usize, usize, usize, usize)> {
    let rest = s.strip_prefix("@@ -")?;
    let (old, rest) = rest.split_once(' ')?;
    let rest = rest.strip_prefix('+')?;
    let (new, tail) = rest.split_once(' ')?;
    fn parse(s: &str) -> Option<(usize, usize)> {
        let mut x = s.split(',');
        let a = x.next()?.parse().ok()?;
        let b = x.next().map(str::parse).transpose().ok()?.unwrap_or(1);
        if x.next().is_some() {
            return None;
        }
        Some((a, b))
    }
    let (os, oc) = parse(old)?;
    let (ns, nc) = parse(new)?;
    if !tail.starts_with("@@") {
        return None;
    }
    Some((os, oc, ns, nc))
}
fn count_suffix(start: usize, count: usize) -> String {
    if count == 1 {
        format!("{start}")
    } else {
        format!("{start},{count}")
    }
}
fn unsupported(lines: &[Line]) -> Option<String> {
    for l in lines {
        let s = l.text.as_str();
        if s.starts_with("GIT binary patch") || s.starts_with("Binary files ") {
            return Some("Binary patches are not supported".into());
        }
        if s.starts_with("similarity index ")
            || s.starts_with("rename from ")
            || s.starts_with("rename to ")
            || s.starts_with("copy from ")
            || s.starts_with("copy to ")
        {
            return Some("Rename and copy patches are not supported".into());
        }
        if s.starts_with("old mode ")
            || (s.starts_with("new mode ") && !s.starts_with("new file mode "))
        {
            return Some("File mode changes cannot be selected by line".into());
        }
    }
    if !lines.iter().any(|l| l.text.starts_with("@@ ")) {
        return Some("Patch has no text hunks (mode-only changes are not supported)".into());
    }
    None
}
fn normalize_file_lifecycle(
    source: &str,
    output: &str,
    selected: &HashSet<usize>,
    reverse: bool,
) -> String {
    let src = split_lines(source);
    let out = split_lines(output);
    let selectable = selectable_lines(source);
    let mut sources = Vec::<(String, bool, bool, usize, usize)>::new();
    let mut start = 0;
    while start < src.len() {
        let end = (start + 1..src.len())
            .find(|&i| src[i].text.starts_with("diff --git "))
            .unwrap_or(src.len());
        let block = &src[start..end];
        let key = block.first().map(|l| l.text.clone()).unwrap_or_default();
        let is_new = block.iter().any(|l| l.text.starts_with("new file mode "));
        let is_deleted = block
            .iter()
            .any(|l| l.text.starts_with("deleted file mode "));
        let total = selectable
            .iter()
            .filter(|&&n| n >= start && n < end)
            .count();
        let chosen = selected.iter().filter(|&&n| n >= start && n < end).count();
        sources.push((key, is_new, is_deleted, total, chosen));
        start = end;
    }
    let mut result = String::new();
    start = 0;
    while start < out.len() {
        let end = (start + 1..out.len())
            .find(|&i| out[i].text.starts_with("diff --git "))
            .unwrap_or(out.len());
        let block = &out[start..end];
        let key = block.first().map(|l| l.text.as_str()).unwrap_or("");
        let Some((_, is_new, is_deleted, total, chosen)) = sources.iter().find(|s| s.0 == key)
        else {
            start = end;
            continue;
        };
        if *chosen == 0 || !block.iter().any(|l| l.text.starts_with("@@ ")) {
            start = end;
            continue;
        }
        let mut lines: Vec<Line> = block.to_vec();
        let mode = if *is_new {
            if reverse && *chosen == *total {
                Some(("new file mode ", "deleted file mode "))
            } else if reverse {
                Some(("new file mode ", ""))
            } else {
                None
            }
        } else if *is_deleted {
            if reverse {
                Some(("deleted file mode ", "new file mode "))
            } else if *chosen == *total {
                None
            } else {
                Some(("deleted file mode ", ""))
            }
        } else {
            None
        };
        if let Some((from, to)) = mode
            && let Some(line) = lines.iter_mut().find(|l| l.text.starts_with(from))
        {
            if to.is_empty() {
                line.text.clear();
            } else {
                line.text = line.text.replacen(from, to, 1);
            }
        }
        // Normalize /dev/null headers according to whether the result creates,
        // deletes, or updates the path. Derive the path from the opposite header.
        let creates = *is_new && !reverse || (*is_deleted && reverse);
        let deletes = *chosen == *total && ((*is_new && reverse) || (*is_deleted && !reverse));
        let old = lines
            .iter()
            .find(|l| l.text.starts_with("--- "))
            .map(|l| l.text.clone());
        let new = lines
            .iter()
            .find(|l| l.text.starts_with("+++ "))
            .map(|l| l.text.clone());
        if creates {
            if let (Some(old), Some(new)) = (old, new) {
                let path = if old.ends_with("/dev/null") {
                    new.trim_start_matches("+++ ").to_owned()
                } else {
                    old.trim_start_matches("--- ").to_owned()
                };
                let bpath = if let Some(path) = path.strip_prefix("a/") {
                    format!("b/{path}")
                } else {
                    path
                };
                for l in &mut lines {
                    if l.text.starts_with("--- ") {
                        l.text = "--- /dev/null".into();
                    } else if l.text.starts_with("+++ ") {
                        l.text = format!("+++ {bpath}");
                    }
                }
            }
        } else if deletes {
            if let (Some(old), Some(new)) = (old, new) {
                let path = if new.ends_with("/dev/null") {
                    old.trim_start_matches("--- ").to_owned()
                } else {
                    new.trim_start_matches("+++ ").to_owned()
                };
                let apath = if let Some(path) = path.strip_prefix("b/") {
                    format!("a/{path}")
                } else {
                    path
                };
                for l in &mut lines {
                    if l.text.starts_with("--- ") {
                        l.text = format!("--- {apath}");
                    } else if l.text.starts_with("+++ ") {
                        l.text = "+++ /dev/null".into();
                    }
                }
            }
        } else {
            if let (Some(old), Some(new)) = (old, new) {
                let a = old.trim_start_matches("--- ");
                let b = new.trim_start_matches("+++ ");
                if a == "/dev/null" {
                    let path = b
                        .strip_prefix("b/")
                        .map(|s| format!("a/{s}"))
                        .unwrap_or_else(|| b.to_owned());
                    for l in &mut lines {
                        if l.text == "--- /dev/null" {
                            l.text = format!("--- {path}");
                        }
                    }
                }
                if b == "/dev/null" {
                    let path = a
                        .strip_prefix("a/")
                        .map(|s| format!("b/{s}"))
                        .unwrap_or_else(|| a.to_owned());
                    for l in &mut lines {
                        if l.text == "+++ /dev/null" {
                            l.text = format!("+++ {path}");
                        }
                    }
                }
            }
        }
        for line in lines {
            if !line.text.is_empty() {
                result.push_str(&line.text);
                result.push_str(line.eol);
            }
        }
        start = end;
    }
    result
}
/// Return physical line indices (zero based) of selectable added/deleted content lines.
pub(crate) fn selectable_lines(patch: &str) -> HashSet<usize> {
    let mut result = HashSet::new();
    let mut in_hunk = false;
    for (i, l) in split_lines(patch).iter().enumerate() {
        if l.text.starts_with("@@ ") {
            in_hunk = true;
            continue;
        }
        if in_hunk && l.text.starts_with("diff --git ") {
            in_hunk = false
        }
        if in_hunk && (l.text.starts_with('+') || l.text.starts_with('-')) {
            result.insert(i);
        }
    }
    result
}

/// Build a patch containing only selected changed lines. `selected` uses zero-based
/// physical line indices into the original patch. `reverse` produces its inverse.
pub(crate) fn selected_patch(
    patch: &str,
    selected: &HashSet<usize>,
    reverse: bool,
) -> Result<String, String> {
    let lines = split_lines(patch);
    if let Some(e) = unsupported(&lines) {
        return Err(e);
    }
    let mut out = String::new();
    let mut i = 0usize;
    let mut hunk_no = 0usize;
    let mut source_cumulative_delta = 0isize;
    let mut selected_cumulative_delta = 0isize;
    while i < lines.len() {
        let line = &lines[i];
        if let Some((os, source_old_count, ns, source_new_count)) = hunk_header(&line.text) {
            hunk_no += 1;
            i += 1;
            let mut body = Vec::<(String, &'static str)>::new();
            let mut old = 0usize;
            let mut new = 0usize;
            let mut source_old = 0usize;
            let mut source_new = 0usize;
            let mut selected_change = false;
            let mut previous_emitted = false;
            while i < lines.len()
                && !lines[i].text.starts_with("@@ ")
                && !lines[i].text.starts_with("diff --git ")
            {
                let l = &lines[i];
                let p = l.text.as_bytes().first().copied();
                match p {
                    Some(b' ') => {
                        source_old += 1;
                        source_new += 1;
                        old += 1;
                        new += 1;
                        body.push((l.text.clone(), l.eol));
                        previous_emitted = true;
                    }
                    Some(b'+') => {
                        source_new += 1;
                        let chosen = selected.contains(&i);
                        selected_change |= chosen;
                        // A '+' line is absent from the preimage. When staging forward,
                        // an unselected addition must disappear, never become context.
                        let text = if chosen {
                            if reverse {
                                format!("-{}", &l.text[1..])
                            } else {
                                l.text.clone()
                            }
                        } else if reverse {
                            format!(" {}", &l.text[1..])
                        } else {
                            i += 1;
                            previous_emitted = false;
                            continue;
                        };
                        if text.starts_with('+') {
                            new += 1
                        } else if text.starts_with('-') {
                            old += 1
                        } else {
                            old += 1;
                            new += 1
                        }
                        body.push((text, l.eol));
                        previous_emitted = true;
                    }
                    Some(b'-') => {
                        source_old += 1;
                        let chosen = selected.contains(&i);
                        selected_change |= chosen;
                        if reverse && !chosen {
                            previous_emitted = false;
                            i += 1;
                            continue;
                        }
                        let text = if chosen {
                            if reverse {
                                format!("+{}", &l.text[1..])
                            } else {
                                l.text.clone()
                            }
                        } else {
                            format!(" {}", &l.text[1..])
                        };
                        if text.starts_with('+') {
                            new += 1
                        } else if text.starts_with('-') {
                            old += 1
                        } else {
                            old += 1;
                            new += 1
                        }
                        body.push((text, l.eol));
                        previous_emitted = true;
                    }
                    Some(b'\\') => {
                        if previous_emitted {
                            body.push((l.text.clone(), l.eol));
                        }
                    }
                    _ => return Err(format!("Unexpected line in hunk {}: {}", hunk_no, l.text)),
                }
                i += 1;
            }
            if source_old != source_old_count || source_new != source_new_count {
                return Err(format!(
                    "Hunk {} line counts do not match its header",
                    hunk_no
                ));
            }
            source_cumulative_delta += source_new_count as isize - source_old_count as isize;
            if !selected_change {
                continue;
            }
            let start_old = if reverse { ns } else { os };
            let (old_count, new_count) = (old, new);
            let target_start = if reverse {
                start_old as isize + selected_cumulative_delta
            } else {
                ns as isize + selected_cumulative_delta
                    - (source_cumulative_delta
                        - (source_new_count as isize - source_old_count as isize))
            }
            .max(0) as usize;
            let header = format!(
                "@@ -{} +{} @@",
                count_suffix(start_old, old_count),
                count_suffix(target_start, new_count)
            );
            // Preserve any section label following @@.
            let suffix = line
                .text
                .find(" @@")
                .and_then(|p| line.text[p + 3..].strip_prefix(' '))
                .unwrap_or("");
            out.push_str(&header);
            if !suffix.is_empty() {
                out.push(' ');
                out.push_str(suffix)
            }
            out.push_str(line.eol);
            for (text, eol) in body {
                out.push_str(&text);
                out.push_str(eol)
            }
            selected_cumulative_delta += new_count as isize - old_count as isize;
            continue;
        }
        if line.text.starts_with("diff --git ") {
            source_cumulative_delta = 0;
            selected_cumulative_delta = 0;
        }
        let mut text = line.text.clone();
        if reverse {
            if let Some(rest) = text.strip_prefix("index ").map(str::to_owned) {
                if let Some((a, b)) = rest.split_once("..") {
                    let (b, tail) = b
                        .split_once(' ')
                        .map(|x| (x.0, Some(x.1)))
                        .unwrap_or((b, None));
                    text = format!("index {b}..{a}");
                    if let Some(tail) = tail {
                        text.push(' ');
                        text.push_str(tail)
                    }
                }
            } else if let Some(rest) = text.strip_prefix("old mode ") {
                text = format!("new mode {rest}")
            } else if let Some(rest) = text.strip_prefix("new mode ") {
                text = format!("old mode {rest}")
            }
        }
        out.push_str(&text);
        out.push_str(line.eol);
        i += 1;
    }
    // Avoid unusable zero-change hunks (e.g. empty selection); caller can handle empty patch.
    Ok(normalize_file_lifecycle(patch, &out, selected, reverse))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        path::PathBuf,
        process::Command,
        sync::atomic::{AtomicUsize, Ordering},
    };
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    struct Repo(PathBuf);
    impl Repo {
        fn new() -> Self {
            let p = std::env::temp_dir().join(format!(
                "grit-diff-lines-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&p).unwrap();
            let r = Self(p);
            r.git(&["init", "-q"]);
            r.git(&["config", "user.email", "test@example.invalid"]);
            r.git(&["config", "user.name", "Test"]);
            r
        }
        fn git(&self, args: &[&str]) -> String {
            let o = Command::new("git")
                .args(args)
                .current_dir(&self.0)
                .output()
                .unwrap();
            assert!(
                o.status.success(),
                "git {:?}: {}",
                args,
                String::from_utf8_lossy(&o.stderr)
            );
            String::from_utf8(o.stdout).unwrap()
        }
        fn apply_cached(&self, p: &str, check: bool) {
            let mut c = Command::new("git");
            c.args(["apply", "--cached", "--unidiff-zero"]);
            if check {
                c.arg("--check");
            }
            let mut child = c
                .current_dir(&self.0)
                .stdin(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .unwrap();
            use std::io::Write;
            child.stdin.take().unwrap().write_all(p.as_bytes()).unwrap();
            let o = child.wait_with_output().unwrap();
            assert!(
                o.status.success(),
                "git apply: {}\npatch:\n{p}",
                String::from_utf8_lossy(&o.stderr)
            );
        }
        fn index(&self, p: &str) -> String {
            self.git(&["show", &format!(":{p}")])
        }
        fn patch(&self, args: &[&str]) -> String {
            self.git(args)
        }
        fn apply(&self, patch: &str, check: bool) {
            self.apply_cached(patch, check);
        }
        fn commit_file(&self, path: &str, content: &str) {
            fs::write(self.0.join(path), content).unwrap();
            self.git(&["add", path]);
            self.git(&["commit", "-qm", "base"]);
        }
    }
    impl Drop for Repo {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    #[test]
    fn partial_mixed_changes_and_reverse_apply_to_index() {
        let r = Repo::new();
        fs::write(
            r.0.join("f.txt"),
            "a\nold\nc\nd\ne\nf\ng\nh\ni\nj\nk\nl\nm\nn\no\np\nq\nr\ns\nt\n",
        )
        .unwrap();
        r.git(&["add", "f.txt"]);
        r.git(&["commit", "-qm", "base"]);
        fs::write(
            r.0.join("f.txt"),
            "a\nnew\nc\nd\ne\nf\ng\nh\ni\nJ\nk\nl\nm\nn\no\np\nq\nr\ns\nt\nextra\n",
        )
        .unwrap();
        let patch = r.git(&["diff", "--no-ext-diff", "--unified=1", "--", "f.txt"]);
        let mut choices: Vec<_> = selectable_lines(&patch).into_iter().collect();
        choices.sort_unstable();
        assert!(choices.len() >= 4);
        let chosen: HashSet<_> = choices.iter().take(2).copied().collect();
        let partial = selected_patch(&patch, &chosen, false).unwrap();
        r.apply_cached(&partial, true);
        r.apply_cached(&partial, false);
        let staged = r.index("f.txt");
        assert!(staged.contains("new\n"));
        assert!(!staged.contains("J\n"));
        assert!(!staged.contains("extra\n"));
        // Invert a selected staged change to unstage it while preserving the full worktree.
        let staged_patch = r.git(&[
            "diff",
            "--cached",
            "--no-ext-diff",
            "--unified=1",
            "--",
            "f.txt",
        ]);
        let first = staged_patch
            .lines()
            .enumerate()
            .find(|(_, s)| s.starts_with('+') && !s.starts_with("+++"))
            .unwrap()
            .0;
        let inverse = selected_patch(&staged_patch, &HashSet::from([first]), true).unwrap();
        r.apply_cached(&inverse, true);
        r.apply_cached(&inverse, false);
        let after = r.index("f.txt");
        assert!(!after.contains("new\n"));
        assert_eq!(
            fs::read_to_string(r.0.join("f.txt")).unwrap(),
            "a\nnew\nc\nd\ne\nf\ng\nh\ni\nJ\nk\nl\nm\nn\no\np\nq\nr\ns\nt\nextra\n"
        );
    }
    #[test]
    fn handles_crlf_and_no_final_newline_and_rejects_unsupported() {
        let patch = "diff --git a/f b/f\r\nindex 1111111..2222222 100644\r\n--- a/f\r\n+++ b/f\r\n@@ -1 +1 @@ label\r\n-old\r\n+new\r\n\\ No newline at end of file\r\n";
        let line = patch
            .lines()
            .enumerate()
            .find(|(_, s)| s.starts_with('+') && !s.starts_with("+++"))
            .unwrap()
            .0;
        let out = selected_patch(patch, &HashSet::from([line]), true).unwrap();
        assert!(
            out.contains("@@ -1 +1,0 @@ label\r\n-new\r\n\\ No newline at end of file\r\n"),
            "{out:?}"
        );
        assert!(
            selected_patch("GIT binary patch\n", &HashSet::new(), false)
                .unwrap_err()
                .contains("Binary")
        );
        assert!(
            selected_patch(
                "diff --git a/a b/b\nsimilarity index 100%\n",
                &HashSet::new(),
                false
            )
            .unwrap_err()
            .contains("Rename")
        );
    }

    fn one(patch: &str, wanted_prefix: char, nth: usize) -> usize {
        patch
            .lines()
            .enumerate()
            .filter(|(_, l)| {
                l.starts_with(wanted_prefix) && !l.starts_with("+++ ") && !l.starts_with("--- ")
            })
            .nth(nth)
            .unwrap()
            .0
    }
    fn apply_selected(repo: &Repo, patch: &str, line: usize, reverse: bool) {
        let p = selected_patch(patch, &HashSet::from([line]), reverse).unwrap();
        repo.apply(&p, true);
        repo.apply(&p, false);
    }
    #[test]
    fn selects_one_of_multiple_additions_forward() {
        let r = Repo::new();
        r.commit_file("f", "head\ntail\n");
        fs::write(r.0.join("f"), "head\none\ntwo\nthree\ntail\n").unwrap();
        let p = r.patch(&["diff", "--no-ext-diff", "--unified=3", "--", "f"]);
        apply_selected(&r, &p, one(&p, '+', 1), false);
        assert_eq!(r.index("f"), "head\ntwo\ntail\n");
    }
    #[test]
    fn selects_one_of_multiple_deletions_forward() {
        let r = Repo::new();
        r.commit_file("f", "head\none\ntwo\nthree\ntail\n");
        fs::write(r.0.join("f"), "head\ntail\n").unwrap();
        let p = r.patch(&["diff", "--no-ext-diff", "--unified=3", "--", "f"]);
        apply_selected(&r, &p, one(&p, '-', 1), false);
        assert_eq!(r.index("f"), "head\none\nthree\ntail\n");
    }
    #[test]
    fn reverse_unstage_one_of_multiple_additions() {
        let r = Repo::new();
        r.commit_file("f", "head\ntail\n");
        fs::write(r.0.join("f"), "head\none\ntwo\nthree\ntail\n").unwrap();
        r.git(&["add", "f"]);
        let p = r.patch(&[
            "diff",
            "--cached",
            "--no-ext-diff",
            "--unified=3",
            "--",
            "f",
        ]);
        apply_selected(&r, &p, one(&p, '+', 1), true);
        assert_eq!(r.index("f"), "head\none\nthree\ntail\n");
        assert_eq!(
            fs::read_to_string(r.0.join("f")).unwrap(),
            "head\none\ntwo\nthree\ntail\n"
        );
    }
    #[test]
    fn reverse_unstage_one_of_multiple_deletions() {
        let r = Repo::new();
        r.commit_file("f", "head\none\ntwo\nthree\ntail\n");
        fs::write(r.0.join("f"), "head\ntail\n").unwrap();
        r.git(&["add", "f"]);
        let p = r.patch(&[
            "diff",
            "--cached",
            "--no-ext-diff",
            "--unified=3",
            "--",
            "f",
        ]);
        apply_selected(&r, &p, one(&p, '-', 1), true);
        assert_eq!(r.index("f"), "head\ntwo\ntail\n");
    }
    #[test]
    fn new_file_partial_stage_and_unstage_keeps_file() {
        let r = Repo::new();
        r.commit_file("base", "base\n");
        fs::write(r.0.join("new"), "one\ntwo\nthree\n").unwrap();
        let diff = Command::new("git")
            .args(["diff", "--no-index", "--", "/dev/null", "new"])
            .current_dir(&r.0)
            .output()
            .unwrap();
        assert_eq!(diff.status.code(), Some(1));
        let p = String::from_utf8(diff.stdout).unwrap();
        let adds: Vec<_> = p
            .lines()
            .enumerate()
            .filter(|(_, l)| l.starts_with('+') && !l.starts_with("+++ "))
            .map(|(i, _)| i)
            .collect();
        let partial = selected_patch(&p, &HashSet::from([adds[0], adds[1]]), false).unwrap();
        r.apply(&partial, true);
        r.apply(&partial, false);
        assert_eq!(r.index("new"), "one\ntwo\n");
        let staged = r.patch(&[
            "diff",
            "--cached",
            "--no-ext-diff",
            "--unified=3",
            "--",
            "new",
        ]);
        apply_selected(&r, &staged, one(&staged, '+', 0), true);
        assert!(
            r.git(&["ls-files", "--error-unmatch", "new"])
                .contains("new")
        );
        assert_eq!(r.index("new"), "two\n");
        assert_eq!(
            fs::read_to_string(r.0.join("new")).unwrap(),
            "one\ntwo\nthree\n"
        );
    }
    #[test]
    fn deleted_file_partial_stage_and_unstage_recreates_only_selected_line() {
        let r = Repo::new();
        r.commit_file("gone", "one\ntwo\nthree\n");
        fs::remove_file(r.0.join("gone")).unwrap();
        let p = r.patch(&["diff", "--no-ext-diff", "--unified=3", "--", "gone"]);
        apply_selected(&r, &p, one(&p, '-', 1), false);
        assert_eq!(r.index("gone"), "one\nthree\n");
        r.git(&["add", "gone"]);
        let staged = r.patch(&[
            "diff",
            "--cached",
            "--no-ext-diff",
            "--unified=3",
            "--",
            "gone",
        ]);
        apply_selected(&r, &staged, one(&staged, '-', 0), true);
        assert_eq!(r.index("gone"), "one\n");
    }
    #[test]
    fn selected_lines_in_multiple_files_keep_file_headers_isolated() {
        let r = Repo::new();
        fs::write(r.0.join("a"), "a\n").unwrap();
        fs::write(r.0.join("b"), "b\n").unwrap();
        r.git(&["add", "a", "b"]);
        r.git(&["commit", "-qm", "base2"]);
        fs::write(r.0.join("a"), "a\nA\n").unwrap();
        fs::write(r.0.join("b"), "b\nB\n").unwrap();
        let p = r.patch(&["diff", "--no-ext-diff", "--unified=0"]);
        let adds: Vec<_> = p
            .lines()
            .enumerate()
            .filter(|(_, l)| l.starts_with('+') && !l.starts_with("+++ "))
            .map(|(i, _)| i)
            .collect();
        let partial = selected_patch(&p, &HashSet::from([adds[0], adds[1]]), false).unwrap();
        r.apply(&partial, true);
        r.apply(&partial, false);
        assert_eq!(r.index("a"), "a\nA\n");
        assert_eq!(r.index("b"), "b\nB\n");
    }
    #[test]
    fn later_hunk_uses_index_offsets_when_prior_additions_are_unselected() {
        let r = Repo::new();
        let base = (1..=30).map(|n| format!("line{n}\n")).collect::<String>();
        r.commit_file("f", &base);
        let changed = base
            .replace("line2\n", "line2\nextra-a\nextra-b\n")
            .replace("line25\n", "changed25\n");
        fs::write(r.0.join("f"), changed).unwrap();
        let p = r.patch(&["diff", "--no-ext-diff", "--unified=1", "--", "f"]);
        apply_selected(&r, &p, one(&p, '+', 2), false);
        let staged = r.index("f");
        assert!(staged.contains("changed25\n"));
        assert!(!staged.contains("extra-a\n"));
        assert!(!staged.contains("extra-b\n"));
    }
    #[test]
    fn git_crlf_patch_stages_selected_line_without_normalizing_endings() {
        let r = Repo::new();
        r.commit_file("crlf.txt", "first\r\nold\r\nlast\r\n");
        fs::write(r.0.join("crlf.txt"), "first\r\nnew\r\nlast\r\n").unwrap();
        let p = r.patch(&["diff", "--no-ext-diff", "--unified=1", "--", "crlf.txt"]);
        apply_selected(&r, &p, one(&p, '+', 0), false);
        assert_eq!(r.index("crlf.txt"), "first\r\nold\r\nnew\r\nlast\r\n");
    }
    #[test]
    fn selected_addition_whose_content_starts_with_plus_is_a_body_line() {
        let r = Repo::new();
        r.commit_file("f", "base\n");
        fs::write(r.0.join("f"), "base\n++starts-with-two-pluses\n").unwrap();
        let p = r.patch(&["diff", "--no-ext-diff", "--unified=1", "--", "f"]);
        let index = one(&p, '+', 0);
        assert!(selectable_lines(&p).contains(&index));
        apply_selected(&r, &p, index, false);
        assert_eq!(r.index("f"), "base\n++starts-with-two-pluses\n");
    }
    #[test]
    fn git_patch_without_final_newline_applies_when_both_replacement_lines_are_selected() {
        let r = Repo::new();
        r.commit_file("f", "old");
        fs::write(r.0.join("f"), "new").unwrap();
        let p = r.patch(&["diff", "--no-ext-diff", "--unified=1", "--", "f"]);
        let deletion = one(&p, '-', 0);
        let addition = one(&p, '+', 0);
        let selected = selected_patch(&p, &HashSet::from([deletion, addition]), false).unwrap();
        r.apply(&selected, true);
        r.apply(&selected, false);
        assert_eq!(r.index("f"), "new");
    }
    #[test]
    fn empty_new_file_has_no_selectable_text_and_is_rejected_as_mode_only() {
        let p = "diff --git a/empty b/empty\nnew file mode 100644\nindex 0000000..e69de29\n--- /dev/null\n+++ b/empty\n";
        assert!(selectable_lines(p).is_empty());
        assert!(
            selected_patch(p, &HashSet::new(), false)
                .unwrap_err()
                .contains("hunks")
        );
    }
}
