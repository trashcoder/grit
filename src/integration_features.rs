use super::*;
#[cfg(unix)]
use std::fs;
use std::{
    ffi::{OsStr, OsString},
    path::Component,
};

#[derive(Default)]
pub(crate) struct IntegrationTools {
    /// Persisted by the root app; the remaining fields are transient UI state.
    pub editor_program: String,
    lfs_pattern: String,
    lfs_untrack_pattern: String,
    lfs_output: String,
    lfs_cache: Option<(PathBuf, LfsAvailability, bool)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum LfsAvailability {
    Available(String),
    Unavailable(String),
}

impl IntegrationTools {
    pub(super) fn invalidate(&mut self) {
        self.lfs_cache = None;
    }

    fn refresh_lfs(&mut self, root: &Path) {
        let available = detect_lfs(root);
        let initialized =
            matches!(available, LfsAvailability::Available(_)) && lfs_is_initialized(root);
        self.lfs_cache = Some((root.to_path_buf(), available, initialized));
    }
}

fn detect_lfs(repo: &Path) -> LfsAvailability {
    detect_lfs_with(OsStr::new("git"), repo)
}

fn detect_lfs_with(program: &OsStr, repo: &Path) -> LfsAvailability {
    match Command::new(program)
        .args(["lfs", "version"])
        .current_dir(repo)
        .output()
    {
        Ok(output) if output.status.success() => {
            LfsAvailability::Available(String::from_utf8_lossy(&output.stdout).trim().to_owned())
        }
        Ok(output) => {
            let detail = String::from_utf8_lossy(&output.stderr).trim().to_owned();
            LfsAvailability::Unavailable(if detail.is_empty() {
                "Git LFS is not available for this repository.".into()
            } else {
                format!("Git LFS is unavailable: {detail}")
            })
        }
        Err(error) => LfsAvailability::Unavailable(format!("Git LFS is unavailable: {error}")),
    }
}

fn lfs_is_initialized(repo: &Path) -> bool {
    Command::new("git")
        .args(["config", "--local", "--get", "filter.lfs.process"])
        .current_dir(repo)
        .output()
        .is_ok_and(|output| output.status.success())
}

#[cfg(test)]
fn unavailable_message(state: &LfsAvailability) -> Option<&str> {
    match state {
        LfsAvailability::Unavailable(message) => Some(message),
        LfsAvailability::Available(_) => None,
    }
}

fn valid_lfs_pattern(pattern: &str) -> bool {
    !pattern.trim().is_empty()
        && !pattern.contains('\n')
        && !pattern.contains('\r')
        && !pattern.contains('\0')
}

fn lfs_track_args(pattern: &str) -> Result<Vec<String>, String> {
    if !valid_lfs_pattern(pattern) {
        return Err("Enter one non-empty LFS pattern without line breaks".into());
    }
    Ok(vec![
        "lfs".into(),
        "track".into(),
        "--".into(),
        pattern.into(),
    ])
}

fn lfs_untrack_args(pattern: &str) -> Result<Vec<String>, String> {
    if !valid_lfs_pattern(pattern) {
        return Err("Enter one non-empty LFS pattern without line breaks".into());
    }
    Ok(vec![
        "lfs".into(),
        "untrack".into(),
        "--".into(),
        pattern.into(),
    ])
}

/// Return an existing regular repository file after proving its canonical path stays in the worktree.
fn confined_repo_file(root: &Path, relative: &str) -> Result<PathBuf, String> {
    let path = Path::new(relative);
    if relative.is_empty()
        || path.is_absolute()
        || path.components().any(|c| {
            matches!(
                c,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err("Select a file inside the repository".into());
    }
    let canonical_root = root
        .canonicalize()
        .map_err(|e| format!("Could not resolve repository: {e}"))?;
    let candidate = canonical_root.join(path);
    let canonical_file = candidate
        .canonicalize()
        .map_err(|e| format!("Could not resolve selected repository file: {e}"))?;
    if !canonical_file.starts_with(&canonical_root) || !canonical_file.is_file() {
        return Err("Selected file must be a regular file inside the repository".into());
    }
    Ok(canonical_file)
}

fn platform_file_manager(root: &Path) -> Command {
    #[cfg(target_os = "windows")]
    {
        let mut c = Command::new("explorer.exe");
        c.arg(root);
        c
    }
    #[cfg(target_os = "macos")]
    {
        let mut c = Command::new("open");
        c.arg(root);
        c
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let mut c = Command::new("xdg-open");
        c.arg(root);
        c
    }
}

fn platform_terminal(root: &Path) -> Result<Command, String> {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        let mut c = Command::new("cmd.exe");
        c.current_dir(root);
        c.creation_flags(0x00000010); // CREATE_NEW_CONSOLE
        Ok(c)
    }
    #[cfg(target_os = "macos")]
    {
        let mut c = Command::new("open");
        c.args(["-a", "Terminal"]).arg(root);
        Ok(c)
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let candidates = [
            "x-terminal-emulator",
            "konsole",
            "kgx",
            "gnome-terminal",
            "kitty",
            "alacritty",
            "xterm",
        ];
        let Some(name) = first_available_terminal(candidates, executable_in_path) else {
            return Err("No supported terminal was found (x-terminal-emulator, konsole, kgx, gnome-terminal, kitty, alacritty, xterm).".into());
        };
        let executable = find_executable(name)
            .ok_or_else(|| format!("Terminal executable not found: {name}"))?;
        Ok(linux_terminal_command(&executable, name, root))
    }
}

fn first_available_terminal<'a>(
    candidates: impl IntoIterator<Item = &'a str>,
    mut available: impl FnMut(&str) -> bool,
) -> Option<&'a str> {
    candidates.into_iter().find(|name| available(name))
}

#[cfg(unix)]
fn find_executable(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|directory| directory.join(name))
        .find(|candidate| executable_file(candidate))
}

#[cfg(unix)]
fn executable_in_path(name: &str) -> bool {
    find_executable(name).is_some()
}

#[cfg(unix)]
fn executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.is_file()
        && fs::metadata(path).is_ok_and(|metadata| metadata.permissions().mode() & 0o111 != 0)
}

#[cfg(unix)]
fn linux_terminal_command(executable: &Path, name: &str, root: &Path) -> Command {
    let mut command = Command::new(executable);
    command.current_dir(root);
    match name {
        "konsole" => {
            command.arg("--workdir").arg(root);
        }
        "kgx" => {
            command.arg(format!("--working-directory={}", root.display()));
        }
        "gnome-terminal" => {
            command.arg(format!("--working-directory={}", root.display()));
        }
        "kitty" => {
            command.arg("--directory").arg(root);
        }
        "alacritty" => {
            command.arg("--working-directory").arg(root);
        }
        // x-terminal-emulator and xterm inherit the working directory from the child process.
        _ => {}
    }
    command
}

fn editor_arguments(root: &Path, relative: &str) -> Result<Vec<OsString>, String> {
    let canonical_file = confined_repo_file(root, relative)?;
    Ok(vec![canonical_file.into_os_string()])
}

impl GritApp {
    pub(super) fn integrations_view(&mut self, ui: &mut egui::Ui) {
        ui.heading("Integrations");
        self.external_diff_tools_ui(ui);
        let Some(root) = self.path.clone() else {
            ui.label("Open a repository to use integrations.");
            return;
        };
        let busy = self.operation.is_some();
        if self
            .integrations
            .lfs_cache
            .as_ref()
            .is_none_or(|(cached_root, _, _)| cached_root != &root)
        {
            self.integrations.refresh_lfs(&root);
        }

        ui.group(|ui| {
            ui.horizontal(|ui| {
                ui.heading("Git LFS");
                if ui.button("Refresh LFS status").clicked() {
                    self.integrations.refresh_lfs(&root);
                }
            });
            let (_, availability, initialized) =
                self.integrations.lfs_cache.as_ref().unwrap().clone();
            match availability {
                LfsAvailability::Unavailable(message) => {
                    ui.colored_label(Color32::YELLOW, message);
                    ui.label("Install Git LFS and restart grit to use these repository tools.");
                }
                LfsAvailability::Available(version) => {
                    ui.label(version);
                    if !initialized {
                        ui.label("Git LFS is available but not initialized for this repository.");
                        if ui
                            .add_enabled(
                                !busy,
                                egui::Button::new("Initialize LFS in this repository"),
                            )
                            .clicked()
                        {
                            self.git_action("Initialize Git LFS", &["lfs", "install", "--local"]);
                        }
                    } else {
                        ui.horizontal(|ui| {
                            ui.add_sized(
                                [160., 24.],
                                egui::TextEdit::singleline(&mut self.integrations.lfs_pattern)
                                    .hint_text("Pattern to track"),
                            );
                            let can_track =
                                !busy && valid_lfs_pattern(&self.integrations.lfs_pattern);
                            if ui
                                .add_enabled(can_track, egui::Button::new("Track pattern"))
                                .clicked()
                            {
                                match lfs_track_args(&self.integrations.lfs_pattern) {
                                    Ok(args) => self.start_git_command("Track LFS pattern", args),
                                    Err(e) => self.notice = e,
                                }
                            }
                        });
                        ui.horizontal(|ui| {
                            ui.add_sized(
                                [160., 24.],
                                egui::TextEdit::singleline(
                                    &mut self.integrations.lfs_untrack_pattern,
                                )
                                .hint_text("Pattern to untrack"),
                            );
                            let can_untrack =
                                !busy && valid_lfs_pattern(&self.integrations.lfs_untrack_pattern);
                            if ui
                                .add_enabled(can_untrack, egui::Button::new("Untrack pattern"))
                                .clicked()
                            {
                                match lfs_untrack_args(&self.integrations.lfs_untrack_pattern) {
                                    Ok(args) => self.start_git_command("Untrack LFS pattern", args),
                                    Err(e) => self.notice = e,
                                }
                            }
                        });
                        ui.horizontal(|ui| {
                            if ui
                                .add_enabled(!busy, egui::Button::new("List tracked patterns"))
                                .clicked()
                            {
                                match Command::new("git")
                                    .args(["lfs", "track"])
                                    .current_dir(&root)
                                    .output()
                                {
                                    Ok(out) if out.status.success() => {
                                        self.integrations.lfs_output =
                                            String::from_utf8_lossy(&out.stdout).trim().to_owned()
                                    }
                                    Ok(out) => {
                                        self.integrations.lfs_output =
                                            String::from_utf8_lossy(&out.stderr).trim().to_owned()
                                    }
                                    Err(e) => {
                                        self.integrations.lfs_output =
                                            format!("Could not list tracked patterns: {e}")
                                    }
                                }
                            }
                            if ui
                                .add_enabled(
                                    !busy,
                                    egui::Button::new("Show cached locks (offline)"),
                                )
                                .clicked()
                            {
                                self.git_action(
                                    "Show cached LFS locks",
                                    &["lfs", "locks", "--cached"],
                                );
                            }
                        });
                        if !self.integrations.lfs_output.is_empty() {
                            ui.label(RichText::new(&self.integrations.lfs_output).monospace());
                        }
                        ui.label("Lock actions contact the configured LFS server.");
                        ui.horizontal(|ui| {
                            if ui
                                .add_enabled(
                                    !busy && self.selected_file.is_some(),
                                    egui::Button::new("Lock selected file"),
                                )
                                .clicked()
                                && let Some(file) = self.selected_file.clone()
                            {
                                match confined_repo_file(&root, &file) {
                                    Ok(_) => self
                                        .git_action("Lock LFS file", &["lfs", "lock", "--", &file]),
                                    Err(e) => self.notice = e,
                                }
                            }
                            if ui
                                .add_enabled(
                                    !busy && self.selected_file.is_some(),
                                    egui::Button::new("Unlock selected file"),
                                )
                                .clicked()
                                && let Some(file) = self.selected_file.clone()
                            {
                                match confined_repo_file(&root, &file) {
                                    Ok(_) => self.git_action(
                                        "Unlock LFS file",
                                        &["lfs", "unlock", "--", &file],
                                    ),
                                    Err(e) => self.notice = e,
                                }
                            }
                        });
                    }
                }
            }
        });

        ui.add_space(8.);
        ui.group(|ui| {
            ui.heading("External tools");
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(!busy, egui::Button::new("Open repository folder"))
                    .clicked()
                {
                    spawn_external(
                        platform_file_manager(&root),
                        &mut self.notice,
                        "file manager",
                    );
                }
                if ui
                    .add_enabled(!busy, egui::Button::new("Open repository terminal"))
                    .clicked()
                {
                    match platform_terminal(&root) {
                        Ok(command) => spawn_external(command, &mut self.notice, "terminal"),
                        Err(error) => self.notice = error,
                    }
                }
            });
            ui.label("External editor executable");
            ui.horizontal(|ui| {
                ui.add_sized(
                    [200., 24.],
                    egui::TextEdit::singleline(&mut self.integrations.editor_program)
                        .hint_text("Program path"),
                );
                if ui
                    .add_enabled(!busy, egui::Button::new("Choose…"))
                    .clicked()
                    && let Some(program) = rfd::FileDialog::new().pick_file()
                {
                    self.integrations.editor_program = program.to_string_lossy().into_owned();
                }
            });
            let can_edit = !busy
                && !self.integrations.editor_program.trim().is_empty()
                && self.selected_file.is_some();
            if ui
                .add_enabled(can_edit, egui::Button::new("Open selected file in editor"))
                .clicked()
                && let Some(relative) = self.selected_file.clone()
            {
                match editor_arguments(&root, &relative) {
                    Ok(args) => {
                        let mut command = Command::new(self.integrations.editor_program.trim());
                        command.args(args).current_dir(&root);
                        spawn_external(command, &mut self.notice, "editor");
                    }
                    Err(e) => self.notice = e,
                }
            }
            if self.selected_file.is_none() {
                ui.label("Select a repository file first.");
            }
        });
    }
}

fn spawn_external(mut command: Command, notice: &mut String, label: &str) {
    match command.spawn() {
        Ok(_) => *notice = format!("Started external {label}"),
        Err(error) => *notice = format!("Could not start external {label}: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lfs_arguments_keep_user_input_as_one_argument() {
        assert_eq!(
            lfs_track_args("*.bin --evil").unwrap(),
            ["lfs", "track", "--", "*.bin --evil"]
        );
        assert_eq!(
            lfs_untrack_args("-pattern").unwrap(),
            ["lfs", "untrack", "--", "-pattern"]
        );
        assert!(lfs_track_args("a\nb").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn linux_terminal_selects_first_available_candidate_and_sets_working_directory() {
        let candidates = ["x-terminal-emulator", "konsole", "kgx", "gnome-terminal"];
        let chosen = first_available_terminal(candidates, |name| name == "konsole");
        assert_eq!(chosen, Some("konsole"));
        let root = Path::new("/tmp/repo path");
        let command = linux_terminal_command(Path::new("/usr/bin/konsole"), "konsole", root);
        assert_eq!(command.get_program(), OsStr::new("/usr/bin/konsole"));
        assert_eq!(command.get_current_dir(), Some(root));
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            [OsStr::new("--workdir"), root.as_os_str()]
        );
        let gnome =
            linux_terminal_command(Path::new("/usr/bin/gnome-terminal"), "gnome-terminal", root);
        assert_eq!(
            gnome.get_args().next(),
            Some(OsStr::new("--working-directory=/tmp/repo path"))
        );
        assert_eq!(first_available_terminal(candidates, |_| false), None);
    }

    #[test]
    fn editor_receives_one_absolute_argument_even_for_dash_prefixed_filename() {
        let root = std::env::temp_dir().join(format!("grit-editor-arg-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("folder")).unwrap();
        fs::write(root.join("folder/-file name.txt"), "data").unwrap();
        let args = editor_arguments(&root, "folder/-file name.txt").unwrap();
        assert_eq!(args.len(), 1);
        assert_eq!(
            PathBuf::from(&args[0]),
            root.join("folder/-file name.txt").canonicalize().unwrap()
        );
        assert!(editor_arguments(&root, "../outside.txt").is_err());
        assert!(editor_arguments(&root, "/outside.txt").is_err());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn editor_path_is_confined_after_symlink_resolution() {
        let base = std::env::temp_dir().join(format!(
            "grit-integrations-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let root = base.join("repo");
        fs::create_dir_all(&root).unwrap();
        let outside = base.join("outside.txt");
        fs::write(root.join("inside.txt"), "in").unwrap();
        fs::write(&outside, "out").unwrap();
        assert!(confined_repo_file(&root, "inside.txt").is_ok());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&outside, root.join("escape.txt")).unwrap();
            assert!(confined_repo_file(&root, "escape.txt").is_err());
        }
        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn unavailable_lfs_is_reported_without_invoking_a_repository() {
        let state = detect_lfs_with(OsStr::new("git-lfs-does-not-exist"), Path::new("."));
        assert!(unavailable_message(&state).is_some());
        assert!(matches!(state, LfsAvailability::Unavailable(_)));
    }

    #[test]
    fn lfs_track_and_untrack_work_in_a_temporary_repository_when_available() {
        if !matches!(detect_lfs(Path::new(".")), LfsAvailability::Available(_)) {
            return;
        }
        let root = std::env::temp_dir().join(format!(
            "grit-lfs-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let init = Command::new("git")
            .args(["init", "-q"])
            .current_dir(&root)
            .status()
            .unwrap();
        assert!(init.success());
        let install = Command::new("git")
            .args(["lfs", "install", "--local"])
            .current_dir(&root)
            .output()
            .unwrap();
        assert!(
            install.status.success(),
            "{}",
            String::from_utf8_lossy(&install.stderr)
        );
        let track = lfs_track_args("*.bin").unwrap();
        let output = Command::new("git")
            .args(&track)
            .current_dir(&root)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let tracked_before = fs::read(root.join(".gitattributes")).unwrap();
        assert!(String::from_utf8_lossy(&tracked_before).contains("*.bin"));
        let output = Command::new("git")
            .args(["lfs", "track"])
            .current_dir(&root)
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(
            tracked_before,
            fs::read(root.join(".gitattributes")).unwrap()
        );
        let untrack = lfs_untrack_args("*.bin").unwrap();
        let output = Command::new("git")
            .args(&untrack)
            .current_dir(&root)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            !String::from_utf8_lossy(&fs::read(root.join(".gitattributes")).unwrap())
                .contains("*.bin")
        );
        let _ = fs::remove_dir_all(root);
    }
}
