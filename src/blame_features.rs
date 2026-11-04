use super::*;
use std::{
    fs,
    io::Read,
    path::Component,
    str,
    sync::mpsc::{self, Receiver, TryRecvError},
};

const MAX_BLAME_BYTES: usize = 4 * 1024 * 1024;
const MAX_BLAME_OUTPUT_BYTES: usize = 128 * 1024 * 1024;
const MAX_BLAME_ERROR_BYTES: usize = 64 * 1024;
type BlameData = (String, Vec<BlameLine>, Vec<BlameCommit>);
type BlameResult = Result<BlameData, String>;
type BlameReceiver = Receiver<BlameResult>;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum BlameHeatMode {
    #[default]
    Age,
    Contribution,
}

#[derive(Clone, Debug)]
pub(super) struct BlameLine {
    pub final_commit_id: git2::Oid,
    pub author: String,
    pub email: String,
    pub committed_time: Option<i64>,
    pub original_line: usize,
    pub current_line: usize,
    pub text: String,
    pub origin_path: String,
}

#[derive(Clone, Debug)]
pub(super) struct BlameCommit {
    pub id: git2::Oid,
    pub author: String,
    pub committed_time: Option<i64>,
    pub line_count: usize,
}

#[derive(Default)]
pub(super) struct BlameState {
    pub path: String,
    pub requested_revision: Option<String>,
    pub version: String,
    pub lines: Vec<BlameLine>,
    pub commits: Vec<BlameCommit>,
    pub error: Option<String>,
    pub loading: bool,
    pub heatmap: bool,
    pub heat_mode: BlameHeatMode,
    pub track_moved_copied: bool,
    heat_colors: Vec<Color32>,
    heat_colors_mode: Option<BlameHeatMode>,
    heat_colors_theme: Option<Color32>,
    receiver: Option<BlameReceiver>,
}

impl BlameState {
    pub(crate) fn persistence_strings(&self) -> Vec<String> {
        vec![
            self.heatmap.to_string(),
            match self.heat_mode {
                BlameHeatMode::Age => "age",
                BlameHeatMode::Contribution => "contribution",
            }
            .into(),
            self.track_moved_copied.to_string(),
        ]
    }

    pub(crate) fn load_persistence_strings(&mut self, values: &[String]) {
        self.heatmap = values.first().is_some_and(|value| value == "true");
        self.heat_mode = match values.get(1).map(String::as_str) {
            Some("contribution") => BlameHeatMode::Contribution,
            _ => BlameHeatMode::Age,
        };
        self.track_moved_copied = values.get(2).is_some_and(|value| value == "true");
    }
}

fn checked_path(path: &str) -> Result<PathBuf, String> {
    file_features::validate_repo_path(path)?;
    Ok(Path::new(path).to_path_buf())
}

fn checked_text(bytes: &[u8]) -> Result<&str, String> {
    if bytes.len() > MAX_BLAME_BYTES {
        return Err("Blame preview is limited to 4 MiB".into());
    }
    if bytes.contains(&0) {
        return Err("Binary files cannot be shown in the blame view".into());
    }
    str::from_utf8(bytes).map_err(|e| format!("File is not valid UTF-8: {e}"))
}

#[derive(Debug)]
struct TrackedBlameLine {
    id: git2::Oid,
    original_line: usize,
    author: String,
    email: String,
    committed_time: Option<i64>,
    origin_path: Option<String>,
}

fn parse_line_porcelain(output: &[u8]) -> Result<Vec<TrackedBlameLine>, String> {
    let output = String::from_utf8_lossy(output);
    let mut result = Vec::new();
    let mut current: Option<TrackedBlameLine> = None;
    for row in output.lines() {
        if let Some(content) = row.strip_prefix('\t') {
            let Some(line) = current.take() else {
                return Err("Git returned malformed blame output".into());
            };
            let _ = content;
            result.push(line);
            continue;
        }
        let mut fields = row.split_ascii_whitespace();
        let hash = fields.next();
        let original = fields.next();
        let final_line = fields.next();
        let header_hash = hash
            .unwrap_or_default()
            .strip_prefix('^')
            .unwrap_or(hash.unwrap_or_default());
        if let (Some(original), Some(_final)) = (original, final_line)
            && header_hash.len() >= 40
            && header_hash.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            let id =
                git2::Oid::from_str(header_hash).map_err(|error| error.message().to_owned())?;
            let original_line = original
                .parse::<usize>()
                .map_err(|_| "Git returned malformed blame line number".to_owned())?;
            current = Some(TrackedBlameLine {
                id,
                original_line,
                author: "Unknown author".into(),
                email: String::new(),
                committed_time: None,
                origin_path: None,
            });
            continue;
        }
        if let Some(line) = current.as_mut() {
            if let Some(author) = row.strip_prefix("author ") {
                line.author = author.to_owned();
            } else if let Some(email) = row.strip_prefix("author-mail <") {
                line.email = email.strip_suffix('>').unwrap_or(email).to_owned();
            } else if let Some(time) = row.strip_prefix("author-time ") {
                line.committed_time = time.parse().ok();
            } else if let Some(path) = row.strip_prefix("filename ") {
                line.origin_path = Some(path.to_owned());
            }
        }
    }
    if current.is_some() {
        return Err("Git returned incomplete blame output".into());
    }
    Ok(result)
}

fn git_blame_moved_copied(
    root: &Path,
    path: &str,
    content: &[u8],
    base: git2::Oid,
    historical: bool,
) -> Result<Vec<TrackedBlameLine>, String> {
    let mut command = Command::new("git");
    command.env("LC_ALL", "C").current_dir(root).args([
        "-c",
        "core.quotePath=false",
        "blame",
        "--line-porcelain",
        "-M",
        "-C",
        "-C",
        "-C",
    ]);
    if !historical {
        command.args(["--contents", "-"]);
    }
    command
        .arg(base.to_string())
        .arg("--")
        // `--` ends option parsing and treats this as a path. Do not prefix
        // `:(literal)`: git blame interprets it as part of the filename here.
        .arg(path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|error| format!("Could not start Git blame: {error}"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or("Could not read Git blame output")?;
    let stderr = child
        .stderr
        .take()
        .ok_or("Could not read Git blame errors")?;
    let stdout_reader = std::thread::spawn(move || read_capped(stdout, MAX_BLAME_OUTPUT_BYTES));
    let stderr_reader = std::thread::spawn(move || read_capped(stderr, MAX_BLAME_ERROR_BYTES));
    // Git can reject the repository/revision before it consumes stdin. Keep the
    // pipe error until after wait/stderr collection so this race does not hide
    // the actual Git failure behind a generic broken-pipe message.
    let write_error = if !historical {
        child
            .stdin
            .take()
            .ok_or("Could not send working copy to Git blame")?
            .write_all(content)
            .err()
    } else {
        drop(child.stdin.take());
        None
    };
    let status = child
        .wait()
        .map_err(|error| format!("Could not read Git blame result: {error}"))?;
    let (stdout, stdout_truncated) = stdout_reader
        .join()
        .map_err(|_| "Could not read Git blame output")?
        .map_err(|error| format!("Could not read Git blame output: {error}"))?;
    let (stderr, _) = stderr_reader
        .join()
        .map_err(|_| "Could not read Git blame errors")?
        .map_err(|error| format!("Could not read Git blame errors: {error}"))?;
    if stdout_truncated {
        return Err("Git blame output exceeded the 128 MiB safety limit".into());
    }
    if !status.success() {
        return Err(format!(
            "Git could not track moved/copied lines: {}",
            String::from_utf8_lossy(&stderr).trim()
        ));
    }
    if let Some(error) = write_error {
        return Err(format!("Could not send working copy to Git blame: {error}"));
    }
    let parsed = parse_line_porcelain(&stdout)?;
    let expected_lines = checked_text(content)?.lines().count();
    if parsed.len() != expected_lines {
        return Err("Git returned incomplete moved/copied blame data".into());
    }
    Ok(parsed)
}

fn read_capped(mut reader: impl Read, limit: usize) -> std::io::Result<(Vec<u8>, bool)> {
    let mut retained = Vec::with_capacity(limit.min(1024 * 1024));
    let mut truncated = false;
    let mut buffer = [0u8; 8192];
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        let remaining = limit.saturating_sub(retained.len());
        let keep = remaining.min(count);
        retained.extend_from_slice(&buffer[..keep]);
        truncated |= keep < count;
    }
    Ok((retained, truncated))
}

fn mix_color(start: Color32, end: Color32, amount: f32) -> Color32 {
    let amount = amount.clamp(0.0, 1.0);
    let mix = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * amount).round() as u8;
    Color32::from_rgb(
        mix(start.r(), end.r()),
        mix(start.g(), end.g()),
        mix(start.b(), end.b()),
    )
}

#[cfg(test)]
fn heat_color(
    line: &BlameLine,
    lines: &[BlameLine],
    commits: &[BlameCommit],
    mode: BlameHeatMode,
) -> Color32 {
    if line.final_commit_id.is_zero() {
        return theme_color([45, 83, 125], [194, 224, 251]);
    }
    match mode {
        BlameHeatMode::Age => {
            let times = lines.iter().filter_map(|line| line.committed_time);
            let (oldest, newest) = times.fold((i64::MAX, i64::MIN), |(old, new), time| {
                (old.min(time), new.max(time))
            });
            let amount = if oldest == i64::MAX || oldest == newest {
                0.5
            } else {
                (line.committed_time.unwrap_or(oldest) - oldest) as f32 / (newest - oldest) as f32
            };
            mix_color(
                theme_color([102, 54, 51], [250, 218, 210]),
                theme_color([43, 82, 61], [211, 240, 218]),
                amount,
            )
        }
        BlameHeatMode::Contribution => {
            let max_count = commits
                .iter()
                .filter(|commit| !commit.id.is_zero())
                .map(|commit| commit.line_count)
                .max()
                .unwrap_or(1);
            let count = commits
                .iter()
                .find(|commit| commit.id == line.final_commit_id)
                .map(|commit| commit.line_count)
                .unwrap_or(1);
            let amount = count as f32 / max_count as f32;
            mix_color(
                theme_color([48, 55, 68], [235, 239, 247]),
                theme_color([56, 76, 126], [177, 201, 238]),
                amount,
            )
        }
    }
}

fn heat_colors(lines: &[BlameLine], commits: &[BlameCommit], mode: BlameHeatMode) -> Vec<Color32> {
    let oldest_newest = lines
        .iter()
        .filter_map(|line| line.committed_time)
        .fold((i64::MAX, i64::MIN), |(old, new), time| {
            (old.min(time), new.max(time))
        });
    let max_count = commits
        .iter()
        .filter(|commit| !commit.id.is_zero())
        .map(|commit| commit.line_count)
        .max()
        .unwrap_or(1);
    let counts: HashMap<_, _> = commits
        .iter()
        .map(|commit| (commit.id, commit.line_count))
        .collect();
    lines
        .iter()
        .map(|line| {
            if line.final_commit_id.is_zero() {
                return theme_color([45, 83, 125], [194, 224, 251]);
            }
            match mode {
                BlameHeatMode::Age => {
                    let (oldest, newest) = oldest_newest;
                    let amount = if oldest == i64::MAX || oldest == newest {
                        0.5
                    } else {
                        (line.committed_time.unwrap_or(oldest) - oldest) as f32
                            / (newest - oldest) as f32
                    };
                    mix_color(
                        theme_color([102, 54, 51], [250, 218, 210]),
                        theme_color([43, 82, 61], [211, 240, 218]),
                        amount,
                    )
                }
                BlameHeatMode::Contribution => {
                    let count = counts.get(&line.final_commit_id).copied().unwrap_or(1);
                    mix_color(
                        theme_color([48, 55, 68], [235, 239, 247]),
                        theme_color([56, 76, 126], [177, 201, 238]),
                        count as f32 / max_count as f32,
                    )
                }
            }
        })
        .collect()
}

fn current_bytes(root: &Path, relative: &Path) -> Result<Vec<u8>, String> {
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    let mut cursor = root.clone();
    for component in relative.components() {
        let Component::Normal(part) = component else {
            return Err("File path must stay inside the repository".into());
        };
        cursor.push(part);
        let metadata = fs::symlink_metadata(&cursor).map_err(|e| e.to_string())?;
        if metadata.file_type().is_symlink() {
            return Err("Symbolic links are not supported in the blame view".into());
        }
    }
    let canonical = cursor.canonicalize().map_err(|e| e.to_string())?;
    if !canonical.starts_with(&root) {
        return Err("File path resolves outside the repository".into());
    }
    let metadata = fs::metadata(&canonical).map_err(|e| e.to_string())?;
    if !metadata.is_file() {
        return Err("Blame requires a regular file".into());
    }
    if metadata.len() > MAX_BLAME_BYTES as u64 {
        return Err("Blame preview is limited to 4 MiB".into());
    }
    fs::read(canonical).map_err(|e| e.to_string())
}

fn blame_data(
    repo: &Repository,
    path: &str,
    revision: Option<&str>,
    track_moved_copied: bool,
) -> BlameResult {
    let relative = checked_path(path)?;
    let root = repo
        .workdir()
        .ok_or("Blame requires a repository with a worktree")?;
    let (content, base, version, historical) = if let Some(revision) = revision {
        let object = repo
            .revparse_single(revision)
            .map_err(|e| format!("Invalid revision: {e}"))?;
        let commit = object
            .peel_to_commit()
            .map_err(|e| format!("Revision is not a commit: {e}"))?;
        let tree = commit.tree().map_err(|e| e.to_string())?;
        let entry = tree
            .get_path(&relative)
            .map_err(|e| format!("File is absent at this revision: {e}"))?;
        if entry.filemode() == 0o120000 {
            return Err("Symbolic links are not supported in the blame view".into());
        }
        let blob = repo.find_blob(entry.id()).map_err(|e| e.to_string())?;
        checked_text(blob.content())?;
        (
            blob.content().to_vec(),
            commit.id(),
            format!("Revision {} ({})", &commit.id().to_string()[..8], revision),
            true,
        )
    } else {
        let content = current_bytes(root, &relative)?;
        checked_text(&content)?;
        let head = repo
            .head()
            .and_then(|head| head.peel_to_commit())
            .map_err(|e| format!("Could not resolve HEAD: {e}"))?;
        (
            content,
            head.id(),
            "Working copy (compared with HEAD)".into(),
            false,
        )
    };

    let mut options = git2::BlameOptions::new();
    options.newest_commit(base);
    let original_blame = repo
        .blame_file(&relative, Some(&mut options))
        .map_err(|e| format!("Could not blame file: {e}"))?;
    let blamed = if historical {
        None
    } else {
        Some(
            original_blame
                .blame_buffer(&content)
                .map_err(|e| format!("Could not blame working copy: {e}"))?,
        )
    };
    let blame = blamed.as_ref().unwrap_or(&original_blame);
    let tracked = if track_moved_copied {
        Some(git_blame_moved_copied(
            root, path, &content, base, historical,
        )?)
    } else {
        None
    };
    let text = checked_text(&content)?;
    let raw_lines: Vec<&str> = text.lines().collect();
    let mut lines = Vec::with_capacity(raw_lines.len());
    for (index, line) in raw_lines.iter().enumerate() {
        let current_line = index + 1;
        let hunk = blame
            .get_line(current_line)
            .ok_or_else(|| format!("No blame data for line {current_line}"))?;
        let id = tracked
            .as_ref()
            .and_then(|tracked| tracked.get(index))
            .map(|line| line.id)
            .unwrap_or_else(|| hunk.final_commit_id());
        let id = if !historical && hunk.final_commit_id().is_zero() {
            git2::Oid::zero()
        } else {
            id
        };
        let signature = hunk.final_signature();
        let tracked_line = tracked.as_ref().and_then(|tracked| tracked.get(index));
        let (author, email, committed_time) = if id.is_zero() {
            ("Uncommitted changes".into(), String::new(), None)
        } else if let Some(tracked_line) = tracked_line {
            (
                tracked_line.author.clone(),
                tracked_line.email.clone(),
                tracked_line.committed_time,
            )
        } else {
            (
                signature.name().unwrap_or("Unknown author").to_owned(),
                signature.email().unwrap_or("").to_owned(),
                Some(signature.when().seconds()),
            )
        };
        let original_line = if id.is_zero() {
            0
        } else if let Some(tracked_line) = tracked_line {
            tracked_line.original_line
        } else {
            hunk.orig_start_line() + current_line - hunk.final_start_line()
        };
        lines.push(BlameLine {
            final_commit_id: id,
            author,
            email,
            committed_time,
            original_line,
            current_line,
            text: (*line).to_owned(),
            origin_path: tracked_line
                .and_then(|line| line.origin_path.clone())
                .unwrap_or_else(|| path.to_owned()),
        });
    }
    let mut commits: Vec<BlameCommit> = Vec::new();
    for line in &lines {
        if let Some(existing) = commits
            .iter_mut()
            .find(|commit| commit.id == line.final_commit_id)
        {
            existing.line_count += 1;
        } else {
            commits.push(BlameCommit {
                id: line.final_commit_id,
                author: line.author.clone(),
                committed_time: line.committed_time,
                line_count: 1,
            });
        }
    }
    commits.sort_by_key(|commit| std::cmp::Reverse(commit.committed_time));
    Ok((version, lines, commits))
}

impl GritApp {
    pub(super) fn open_blame(&mut self, path: String, revision: Option<String>) {
        let heatmap = self.blame_state.heatmap;
        let heat_mode = self.blame_state.heat_mode;
        let track_moved_copied = self.blame_state.track_moved_copied;
        self.blame_state = BlameState {
            path: path.clone(),
            requested_revision: revision.clone(),
            loading: true,
            heatmap,
            heat_mode,
            track_moved_copied,
            ..Default::default()
        };
        let Some(repo_path) = self.path.clone() else {
            self.blame_state.loading = false;
            self.blame_state.error = Some("Open a repository first".into());
            self.tab = 4;
            return;
        };
        let (sender, receiver) = mpsc::channel();
        self.blame_state.receiver = Some(receiver);
        std::thread::spawn(move || {
            let result = Repository::open(repo_path)
                .map_err(|e| e.to_string())
                .and_then(|repo| blame_data(&repo, &path, revision.as_deref(), track_moved_copied));
            let _ = sender.send(result);
        });
        self.tab = 4;
    }

    pub(super) fn blame_workspace(&mut self, ui: &mut egui::Ui) {
        if let Some(receiver) = self.blame_state.receiver.take() {
            match receiver.try_recv() {
                Ok(Ok((version, lines, commits))) => {
                    self.blame_state.loading = false;
                    self.blame_state.version = version;
                    self.blame_state.lines = lines;
                    self.blame_state.commits = commits;
                    self.blame_state.heat_colors.clear();
                    self.blame_state.heat_colors_mode = None;
                    self.blame_state.heat_colors_theme = None;
                }
                Ok(Err(error)) => {
                    self.blame_state.loading = false;
                    self.blame_state.error = Some(error);
                }
                Err(TryRecvError::Disconnected) => {
                    self.blame_state.loading = false;
                    self.blame_state.error = Some("Blame worker stopped unexpectedly".into());
                }
                Err(TryRecvError::Empty) => {
                    self.blame_state.receiver = Some(receiver);
                    ui.ctx()
                        .request_repaint_after(std::time::Duration::from_millis(100));
                }
            }
        }
        ui.heading(format!("Blame · {}", self.blame_state.path));
        let mut refresh = false;
        let mut tracking_changed = false;
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.blame_state.heatmap, "Heatmap");
            egui::ComboBox::from_id_salt("blame_heat_mode")
                .selected_text(match self.blame_state.heat_mode {
                    BlameHeatMode::Age => "By age",
                    BlameHeatMode::Contribution => "By contribution",
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut self.blame_state.heat_mode,
                        BlameHeatMode::Age,
                        "By age",
                    );
                    ui.selectable_value(
                        &mut self.blame_state.heat_mode,
                        BlameHeatMode::Contribution,
                        "By contribution",
                    );
                });
            tracking_changed = ui
                .add_enabled(
                    !self.blame_state.loading,
                    egui::Checkbox::new(
                        &mut self.blame_state.track_moved_copied,
                        "Track moved/copied lines",
                    ),
                )
                .on_hover_text("Uses Git's -M -C -C -C blame passes. Copy tracking searches history and may take longer.")
                .changed();
            if tracking_changed {
                refresh = true;
            }
        });
        ui.horizontal(|ui| {
            ui.label(RichText::new(&self.blame_state.version).color(muted()));
            if ui.button("Refresh").clicked() && !self.blame_state.loading {
                refresh = true;
            }
            if self.blame_state.loading {
                ui.spinner();
                ui.label("Loading blame…");
            }
        });
        if refresh {
            let path = self.blame_state.path.clone();
            let revision = self.blame_state.requested_revision.clone();
            self.open_blame(path, revision);
        }
        if let Some(error) = &self.blame_state.error {
            ui.colored_label(Color32::LIGHT_RED, error);
            return;
        }
        if self.blame_state.loading {
            return;
        }
        if self.blame_state.track_moved_copied {
            ui.label("Moved/copied line tracking uses Git's similarity heuristics (20 alphanumeric characters within a file; 40 across files by default). Results can miss shorter or substantially edited blocks.");
        }
        if self.blame_state.lines.is_empty() {
            ui.label("No lines to annotate.");
            return;
        }
        if self.blame_state.heatmap {
            let theme_key = theme_color([0, 0, 0], [255, 255, 255]);
            if self.blame_state.heat_colors_mode != Some(self.blame_state.heat_mode)
                || self.blame_state.heat_colors_theme != Some(theme_key)
                || self.blame_state.heat_colors.len() != self.blame_state.lines.len()
            {
                self.blame_state.heat_colors = heat_colors(
                    &self.blame_state.lines,
                    &self.blame_state.commits,
                    self.blame_state.heat_mode,
                );
                self.blame_state.heat_colors_mode = Some(self.blame_state.heat_mode);
                self.blame_state.heat_colors_theme = Some(theme_key);
            }
        }
        let mut selected_commit = None;
        egui::SidePanel::left("blame_timeline_panel")
            .resizable(true)
            .default_width(250.)
            .width_range(180. ..=360.)
            .show_inside(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("blame_timeline_scroll")
                    .show(ui, |ui| {
                        ui.strong("Commits in this file");
                        for commit in &self.blame_state.commits {
                            let label = if commit.id.is_zero() {
                                format!("Uncommitted · {} lines", commit.line_count)
                            } else {
                                let date = commit
                                    .committed_time
                                    .and_then(|time| {
                                        chrono::DateTime::<chrono::Utc>::from_timestamp(time, 0)
                                    })
                                    .map(|date| date.format("%Y-%m-%d").to_string())
                                    .unwrap_or_else(|| "Unknown date".into());
                                format!(
                                    "{} · {} lines\n{} · {}",
                                    date,
                                    commit.line_count,
                                    commit.author,
                                    &commit.id.to_string()[..8]
                                )
                            };
                            if ui.selectable_label(false, label).clicked() && !commit.id.is_zero() {
                                selected_commit = Some(commit.id.to_string());
                            }
                        }
                    });
            });
        egui::CentralPanel::default().show_inside(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.;
            let row_height = ui
                .text_style_height(&egui::TextStyle::Monospace)
                .max(ui.text_style_height(&egui::TextStyle::Body))
                + 4.0;
            ui.spacing_mut().interact_size.y = row_height;
            egui::ScrollArea::both()
                .id_salt("blame_lines_scroll")
                .show_rows(
                    ui,
                    row_height,
                    self.blame_state.lines.len(),
                    |ui, visible_rows| {
                        for row in visible_rows {
                            if let Some(line) = self.blame_state.lines.get(row) {
                                ui.horizontal(|ui| {
                                    let date = line
                                        .committed_time
                                        .and_then(|time| {
                                            chrono::DateTime::<chrono::Utc>::from_timestamp(time, 0)
                                        })
                                        .map(|date| date.format("%Y-%m-%d").to_string())
                                        .unwrap_or_else(|| "Uncommitted".into());
                                    let id = if line.final_commit_id.is_zero() {
                                        "working".into()
                                    } else {
                                        line.final_commit_id.to_string()[..8].to_owned()
                                    };
                                    let mut metadata = RichText::new(format!(
                                        "{date} {id} {:>5}/{:>5} {}",
                                        line.original_line, line.current_line, line.author
                                    ))
                                    .monospace();
                                    if let Some(color) = self.blame_state.heat_colors.get(row)
                                        && self.blame_state.heatmap
                                    {
                                        metadata = metadata.background_color(*color);
                                    }
                                    let response = ui.add_sized(
                                        [360., row_height],
                                        egui::Label::new(metadata)
                                            .truncate()
                                            .sense(egui::Sense::click()),
                                    );
                                    let response = response.on_hover_text(format!(
                                        "{} <{}>\nOrigin (Git path): {}",
                                        line.author, line.email, line.origin_path
                                    ));
                                    if !line.final_commit_id.is_zero() && response.clicked() {
                                        selected_commit = Some(line.final_commit_id.to_string());
                                    }
                                    ui.add(
                                        egui::Label::new(RichText::new(&line.text).monospace())
                                            .extend(),
                                    );
                                });
                            }
                        }
                    },
                );
        });
        if let Some(id) = selected_commit {
            self.search.clear();
            self.history_options.query_cache = None;
            self.history_options.only_starred = false;
            self.history_options.first_parent = false;
            self.set_history_scope(None);
            self.select_history_commit(id);
            self.tab = 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct TestDir(PathBuf);

    impl TestDir {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "grit-blame-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn commit(
        repo: &Repository,
        path: &Path,
        content: &[u8],
        name: &str,
        email: &str,
        message: &str,
    ) -> git2::Oid {
        fs::write(path, content).unwrap();
        let mut index = repo.index().unwrap();
        index
            .add_path(Path::new(path.file_name().unwrap()))
            .unwrap();
        index.write().unwrap();
        let tree_id = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let signature = git2::Signature::now(name, email).unwrap();
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

    #[test]
    fn real_blame_tracks_authors_revisions_and_working_changes() {
        let dir = TestDir::new();
        let repo = Repository::init(dir.path()).unwrap();
        let path = dir.path().join("file [x]*.txt");
        let first = commit(
            &repo,
            &path,
            b"alpha\nbeta\n",
            "Ada",
            "ada@example.test",
            "first",
        );
        let second = commit(
            &repo,
            &path,
            b"alpha\nchanged\n",
            "Grace",
            "grace@example.test",
            "second",
        );
        let (version, lines, _) = blame_data(&repo, "file [x]*.txt", None, false).unwrap();
        assert!(version.starts_with("Working copy"));
        assert_eq!(
            lines
                .iter()
                .map(|line| line.final_commit_id)
                .collect::<Vec<_>>(),
            [first, second]
        );
        assert_eq!(lines[0].original_line, 1);
        assert_eq!(lines[1].current_line, 2);
        let (_, old, _) =
            blame_data(&repo, "file [x]*.txt", Some(&first.to_string()), false).unwrap();
        assert_eq!(old.len(), 2);
        assert!(old.iter().all(|line| line.final_commit_id == first));
        fs::write(&path, b"alpha\nchanged\npending\n").unwrap();
        let (_, dirty, commits) = blame_data(&repo, "file [x]*.txt", None, false).unwrap();
        let (_, dirty_with_tracking, _) = blame_data(&repo, "file [x]*.txt", None, true).unwrap();
        assert!(dirty[2].final_commit_id.is_zero());
        assert!(dirty_with_tracking[2].final_commit_id.is_zero());
        assert_eq!(dirty[2].author, "Uncommitted changes");
        assert!(
            commits
                .iter()
                .any(|commit| commit.id.is_zero() && commit.line_count == 1)
        );
    }

    #[test]
    fn git_copy_tracking_attributes_copied_lines_to_their_source_commit() {
        let dir = TestDir::new();
        let repo = Repository::init(dir.path()).unwrap();
        let source = dir.path().join("söurce.rs");
        let destination = dir.path().join("destination [ä]*.rs");
        let copied = "    let copied_value = calculate_the_stable_source_value(input);\n    validate_the_copied_result_before_returning(copied_value);\n";
        let source_commit = commit(
            &repo,
            &source,
            format!("fn source(input: usize) {{\n{copied}}}\n").as_bytes(),
            "Ada",
            "ada@example.test",
            "add source block",
        );
        fs::write(
            &destination,
            format!("fn destination(input: usize) {{\n{copied}}}\n"),
        )
        .unwrap();
        let destination_commit = commit(
            &repo,
            &destination,
            &fs::read(&destination).unwrap(),
            "Grace",
            "grace@example.test",
            "copy source block",
        );

        let destination_name = "destination [ä]*.rs";
        let (_, normal, _) = blame_data(&repo, destination_name, None, false).unwrap();
        let (_, tracked, _) = blame_data(&repo, destination_name, None, true).unwrap();
        assert!(
            normal
                .iter()
                .all(|line| line.final_commit_id == destination_commit)
        );
        assert!(
            tracked
                .iter()
                .any(|line| line.final_commit_id == source_commit)
        );
        assert!(
            tracked
                .iter()
                .filter(|line| line.text.contains("copied") || line.text.contains("validate"))
                .all(|line| line.final_commit_id == source_commit)
        );
        assert!(tracked.iter().any(|line| line.origin_path == "söurce.rs"));
    }

    #[test]
    fn git_move_tracking_reports_the_original_filename() {
        let dir = TestDir::new();
        let repo = Repository::init(dir.path()).unwrap();
        let source = dir.path().join("source.rs");
        let source_commit = commit(
            &repo,
            &source,
            b"fn moved_block(input: usize) {\n    calculate_the_moved_block_from_history(input);\n}\n",
            "Ada",
            "ada@example.test",
            "add block",
        );
        let destination = dir.path().join("moved.rs");
        fs::rename(&source, &destination).unwrap();
        let mut index = repo.index().unwrap();
        index.remove_path(Path::new("source.rs")).unwrap();
        index.write().unwrap();
        let destination_commit = commit(
            &repo,
            &destination,
            &fs::read(&destination).unwrap(),
            "Grace",
            "grace@example.test",
            "move block",
        );

        let (_, tracked, _) = blame_data(&repo, "moved.rs", None, true).unwrap();
        assert!(
            tracked
                .iter()
                .all(|line| line.final_commit_id == source_commit)
        );
        assert!(tracked.iter().any(|line| line.origin_path == "source.rs"));
        assert_ne!(destination_commit, source_commit);
    }

    #[test]
    fn heat_colors_distinguish_uncommitted_and_normalize_constant_ranges() {
        let now = BlameLine {
            final_commit_id: git2::Oid::from_str("1111111111111111111111111111111111111111")
                .unwrap(),
            author: "A".into(),
            email: String::new(),
            committed_time: Some(10),
            original_line: 1,
            current_line: 1,
            text: "line".into(),
            origin_path: "file".into(),
        };
        let mut uncommitted = now.clone();
        uncommitted.final_commit_id = git2::Oid::zero();
        uncommitted.committed_time = None;
        let lines = [now.clone(), uncommitted.clone()];
        let commits = [
            BlameCommit {
                id: now.final_commit_id,
                author: "A".into(),
                committed_time: Some(10),
                line_count: 1,
            },
            BlameCommit {
                id: git2::Oid::zero(),
                author: "Uncommitted changes".into(),
                committed_time: None,
                line_count: 1,
            },
        ];
        assert_ne!(
            heat_color(&now, &lines, &commits, BlameHeatMode::Age),
            heat_color(&uncommitted, &lines, &commits, BlameHeatMode::Age)
        );
        assert_ne!(
            heat_color(&now, &lines, &commits, BlameHeatMode::Contribution),
            heat_color(&uncommitted, &lines, &commits, BlameHeatMode::Contribution)
        );
        assert_ne!(
            heat_color(&now, &lines, &commits, BlameHeatMode::Age),
            Color32::TRANSPARENT
        );
    }

    #[test]
    fn git_output_is_bounded_and_process_errors_are_reported() {
        let input = vec![b'x'; 32];
        let (retained, truncated) = read_capped(input.as_slice(), 8).unwrap();
        assert_eq!(retained, b"xxxxxxxx");
        assert!(truncated);

        let dir = TestDir::new();
        let repo = Repository::init(dir.path()).unwrap();
        let path = dir.path().join("tracked");
        let commit_id = commit(
            &repo,
            &path,
            b"stable line\n",
            "Ada",
            "ada@example.test",
            "initial",
        );
        fs::remove_dir_all(dir.path().join(".git")).unwrap();
        let error =
            git_blame_moved_copied(dir.path(), "tracked", b"stable line\n", commit_id, false)
                .unwrap_err();
        assert!(error.contains("Git could not track moved/copied lines"));
        assert!(error.contains("not a git repository"));
    }

    #[test]
    fn rejects_untracked_symlink_binary_invalid_utf8_and_oversize_files() {
        let dir = TestDir::new();
        let repo = Repository::init(dir.path()).unwrap();
        let tracked = dir.path().join("tracked");
        commit(
            &repo,
            &tracked,
            b"ok\n",
            "Ada",
            "ada@example.test",
            "initial",
        );
        assert!(blame_data(&repo, "new", None, false).is_err());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("tracked", dir.path().join("link")).unwrap();
            assert!(
                blame_data(&repo, "link", None, false)
                    .unwrap_err()
                    .contains("Symbolic links")
            );
        }
        fs::write(&tracked, [0xff, 0xfe]).unwrap();
        assert!(
            blame_data(&repo, "tracked", None, false)
                .unwrap_err()
                .contains("UTF-8")
        );
        fs::write(&tracked, b"binary\0data").unwrap();
        assert!(
            blame_data(&repo, "tracked", None, false)
                .unwrap_err()
                .contains("Binary")
        );
        fs::write(&tracked, vec![b'x'; MAX_BLAME_BYTES + 1]).unwrap();
        assert!(
            blame_data(&repo, "tracked", None, false)
                .unwrap_err()
                .contains("4 MiB")
        );
        assert!(checked_path("../outside").is_err());
    }
}
