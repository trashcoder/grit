use crate::GritApp;
use eframe::egui;
use git2::{BranchType, Oid, Reference, Repository};
use std::{
    collections::HashMap,
    path::Path,
    time::{Duration, Instant},
};

#[derive(Clone, Debug)]
pub struct AutoFetchPreference {
    pub enabled: bool,
    pub prune: bool,
    pub interval_minutes: u32,
}
impl Default for AutoFetchPreference {
    fn default() -> Self {
        Self {
            enabled: false,
            prune: false,
            interval_minutes: 15,
        }
    }
}

#[derive(Default)]
pub struct RemoteTools {
    preferences: HashMap<String, AutoFetchPreference>,
    next_fetch: HashMap<String, Instant>,
    pub selected_remote: String,
    pub selected_branch: String,
    pub selected_tag: String,
    pub tracking_remote_branch: String,
    pub lease_target: String,
    pub lease_expected_oid: String,
    pub pending_push: Option<PendingPush>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PendingPush {
    Lease {
        repo_key: String,
        head_ref: String,
        head_oid: String,
        remote: String,
        remote_url: String,
        source_branch: String,
        source_oid: String,
        target: String,
        expected_oid: String,
    },
    Tag {
        repo_key: String,
        head_ref: String,
        head_oid: String,
        remote: String,
        remote_url: String,
        tag: String,
        tag_oid: String,
    },
}

impl RemoteTools {
    /// Preferences are keyed by the repository's canonical worktree/git-dir path.
    pub fn persistence_strings(&self, repo_key: &str) -> Vec<String> {
        let p = self.preferences.get(repo_key).cloned().unwrap_or_default();
        vec![
            p.enabled.to_string(),
            p.prune.to_string(),
            p.interval_minutes.to_string(),
        ]
    }
    pub fn load_persistence_strings(&mut self, repo_key: &str, values: &[String]) {
        self.preferences.insert(
            repo_key.to_owned(),
            AutoFetchPreference {
                enabled: values.first().is_some_and(|v| v == "true"),
                prune: values.get(1).is_some_and(|v| v == "true"),
                interval_minutes: values
                    .get(2)
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(15)
                    .clamp(1, 1440),
            },
        );
    }
}

impl GritApp {
    pub fn advanced_remotes_ui(&mut self, ui: &mut egui::Ui) {
        let Some(repo) = self.repo() else {
            ui.label("Open a repository to configure remote branches and tags.");
            return;
        };
        let repo_key = repo_key(self.path.as_deref().unwrap_or_else(|| repo.path()));
        let remotes = remote_names(&repo);
        if self.remote_tools.selected_remote.is_empty()
            || !remotes.contains(&self.remote_tools.selected_remote)
        {
            self.remote_tools.selected_remote = remotes.first().cloned().unwrap_or_default();
        }
        let locals = local_branches(&repo);
        if self.remote_tools.selected_branch.is_empty()
            || !locals.contains(&self.remote_tools.selected_branch)
        {
            self.remote_tools.selected_branch = locals.first().cloned().unwrap_or_default();
        }
        let tags = repo
            .tag_names(None)
            .ok()
            .map(|t| t.iter().flatten().map(str::to_owned).collect::<Vec<_>>())
            .unwrap_or_default();
        if self.remote_tools.selected_tag.is_empty()
            || !tags.contains(&self.remote_tools.selected_tag)
        {
            self.remote_tools.selected_tag = tags.first().cloned().unwrap_or_default();
        }
        let tracking = remote_branches(&repo);
        let busy = self.operation.is_some();

        ui.heading("Targeted remote operations");
        ui.horizontal(|ui| {
            ui.label("Remote");
            egui::ComboBox::from_id_salt("remote_tools_remote")
                .selected_text(if self.remote_tools.selected_remote.is_empty() {
                    "No remote"
                } else {
                    &self.remote_tools.selected_remote
                })
                .show_ui(ui, |ui| {
                    for name in &remotes {
                        ui.selectable_value(
                            &mut self.remote_tools.selected_remote,
                            name.clone(),
                            name,
                        );
                    }
                });
        });
        ui.horizontal(|ui| {
            ui.label("Local branch");
            egui::ComboBox::from_id_salt("remote_tools_branch")
                .selected_text(if self.remote_tools.selected_branch.is_empty() {
                    "No branch"
                } else {
                    &self.remote_tools.selected_branch
                })
                .show_ui(ui, |ui| {
                    for name in &locals {
                        ui.selectable_value(
                            &mut self.remote_tools.selected_branch,
                            name.clone(),
                            name,
                        );
                    }
                });
            if ui
                .add_enabled(!busy, egui::Button::new("Push branch"))
                .clicked()
            {
                self.push_branch_to_selected();
            }
        });
        ui.horizontal(|ui| {
            ui.label("Track remote branch");
            egui::ComboBox::from_id_salt("remote_tools_tracking")
                .selected_text(if self.remote_tools.tracking_remote_branch.is_empty() {
                    "Choose remote branch"
                } else {
                    &self.remote_tools.tracking_remote_branch
                })
                .show_ui(ui, |ui| {
                    for name in &tracking {
                        ui.selectable_value(
                            &mut self.remote_tools.tracking_remote_branch,
                            name.clone(),
                            name,
                        );
                    }
                });
            if ui
                .add_enabled(!busy, egui::Button::new("Set upstream"))
                .clicked()
            {
                self.set_selected_upstream();
            }
        });
        ui.horizontal(|ui| {
            ui.label("Tag");
            egui::ComboBox::from_id_salt("remote_tools_tag")
                .selected_text(if self.remote_tools.selected_tag.is_empty() {
                    "No tag"
                } else {
                    &self.remote_tools.selected_tag
                })
                .show_ui(ui, |ui| {
                    for name in &tags {
                        ui.selectable_value(
                            &mut self.remote_tools.selected_tag,
                            name.clone(),
                            name,
                        );
                    }
                });
            if ui
                .add_enabled(!busy, egui::Button::new("Prepare tag push"))
                .clicked()
            {
                self.prepare_tag_push();
            }
        });

        ui.separator();
        ui.label("Force-with-lease (exact remote target and observed tracking OID)");
        ui.horizontal(|ui| {
            ui.label("Remote branch");
            ui.add(
                egui::TextEdit::singleline(&mut self.remote_tools.lease_target).hint_text("main"),
            );
            if ui
                .add_enabled(!busy, egui::Button::new("Read expected OID"))
                .clicked()
            {
                self.read_lease_oid();
            }
        });
        ui.horizontal(|ui| {
            ui.label("Expected OID");
            ui.add(
                egui::TextEdit::singleline(&mut self.remote_tools.lease_expected_oid)
                    .desired_width(300.),
            );
            if ui
                .add_enabled(!busy, egui::Button::new("Prepare lease push"))
                .clicked()
            {
                self.prepare_lease_push();
            }
        });

        let pref = self
            .remote_tools
            .preferences
            .entry(repo_key.clone())
            .or_default();
        ui.separator();
        ui.checkbox(
            &mut pref.enabled,
            "Enable automatic fetch for this repository",
        );
        ui.checkbox(&mut pref.prune, "Prune deleted remote branches");
        ui.add_enabled(
            pref.enabled,
            egui::Slider::new(&mut pref.interval_minutes, 1..=1440).suffix(" min"),
        );
        if !pref.enabled {
            self.remote_tools.next_fetch.remove(&repo_key);
        }

        if let Some(pending) = self.remote_tools.pending_push.clone() {
            ui.separator();
            match pending {
                PendingPush::Lease {
                    repo_key: saved_repo,
                    head_ref,
                    head_oid,
                    remote,
                    remote_url,
                    source_branch,
                    source_oid,
                    target,
                    expected_oid,
                } => {
                    ui.colored_label(egui::Color32::YELLOW, format!("Confirm replacing {remote}:refs/heads/{target} with source branch {source_branch} pinned to {source_oid}. Lease requires the observed target OID {expected_oid}. HEAD: {head_ref} {head_oid}."));
                    ui.horizontal(|ui| {
                        if ui.add_enabled(!busy, egui::Button::new("Confirm exact lease push")).clicked() {
                            let snapshot = PendingPush::Lease { repo_key: saved_repo, head_ref, head_oid, remote, remote_url, source_branch, source_oid, target, expected_oid };
                            if self.pending_is_current(&repo, &repo_key, &snapshot) {
                                if let PendingPush::Lease { remote, source_oid, target, expected_oid, .. } = &snapshot {
                                    match lease_push_args(remote, source_oid, target, expected_oid) {
                                        Ok(args) => self.start_git_command("Force-with-lease push", args),
                                        Err(e) => self.notice = e,
                                    }
                                }
                            } else { self.notice = "Repository, HEAD, source, remote, target, or expected OID changed; prepare again".into(); }
                            self.remote_tools.pending_push = None;
                        }
                        if ui.button("Cancel").clicked() { self.remote_tools.pending_push = None; }
                    });
                }
                PendingPush::Tag {
                    repo_key: saved_repo,
                    head_ref,
                    head_oid,
                    remote,
                    remote_url,
                    tag,
                    tag_oid,
                } => {
                    ui.colored_label(egui::Color32::YELLOW, format!("Confirm pushing tag {tag} ({tag_oid}) to {remote}. HEAD: {head_ref} {head_oid}."));
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(!busy, egui::Button::new("Confirm tag push"))
                            .clicked()
                        {
                            let snapshot = PendingPush::Tag {
                                repo_key: saved_repo,
                                head_ref,
                                head_oid,
                                remote,
                                remote_url,
                                tag,
                                tag_oid,
                            };
                            if self.pending_is_current(&repo, &repo_key, &snapshot) {
                                if let PendingPush::Tag {
                                    remote,
                                    tag,
                                    tag_oid,
                                    ..
                                } = &snapshot
                                {
                                    match tag_push_args(remote, tag, tag_oid) {
                                        Ok(args) => self.start_git_command("Push tag", args),
                                        Err(e) => self.notice = e,
                                    }
                                }
                            } else {
                                self.notice =
                                    "Repository, HEAD, remote, or tag changed; prepare again"
                                        .into();
                            }
                            self.remote_tools.pending_push = None;
                        }
                        if ui.button("Cancel").clicked() {
                            self.remote_tools.pending_push = None;
                        }
                    });
                }
            }
        }
    }

    pub fn poll_auto_fetch(&mut self, ctx: &egui::Context) {
        if self.operation.is_some() {
            return;
        }
        let Some(repo) = self.repo() else {
            return;
        };
        let Some(path) = self.path.as_deref() else {
            return;
        };
        let key = repo_key(path);
        let Some(pref) = self.remote_tools.preferences.get(&key).cloned() else {
            return;
        };
        if !pref.enabled || remote_names(&repo).is_empty() {
            return;
        }
        let now = Instant::now();
        let due = *self
            .remote_tools
            .next_fetch
            .entry(key.clone())
            .or_insert_with(|| {
                now + Duration::from_secs(u64::from(pref.interval_minutes.clamp(1, 1440)) * 60)
            });
        if now >= due {
            let mut args = vec!["fetch".to_owned(), "--all".to_owned()];
            if pref.prune {
                args.push("--prune".into());
            }
            self.start_git_command("Automatic fetch", args);
            self.remote_tools.next_fetch.insert(
                key,
                now + Duration::from_secs(u64::from(pref.interval_minutes.clamp(1, 1440)) * 60),
            );
        }
        ctx.request_repaint_after(Duration::from_secs(2));
    }

    fn push_branch_to_selected(&mut self) {
        let Some(repo) = self.repo() else {
            return;
        };
        let remote = self.remote_tools.selected_remote.clone();
        let branch = self.remote_tools.selected_branch.clone();
        if !valid_remote(&repo, &remote) || !valid_branch(&branch) {
            self.notice = "Choose a valid remote and local branch".into();
            return;
        }
        let Ok(commit) = repo
            .find_branch(&branch, BranchType::Local)
            .and_then(|b| b.into_reference().peel_to_commit())
        else {
            self.notice = "Local source branch not found".into();
            return;
        };
        match branch_push_args(&remote, &commit.id().to_string(), &branch) {
            Ok(args) => self.start_git_command("Push branch", args),
            Err(e) => self.notice = e,
        }
    }
    fn set_selected_upstream(&mut self) {
        let Some(repo) = self.repo() else {
            return;
        };
        let branch = self.remote_tools.selected_branch.clone();
        let upstream = self.remote_tools.tracking_remote_branch.clone();
        match tracking_args(&repo, &upstream, &branch) {
            Ok(args) => self.start_git_command("Set branch upstream", args),
            Err(e) => self.notice = e,
        }
    }
    fn prepare_tag_push(&mut self) {
        let Some(repo) = self.repo() else {
            return;
        };
        let tag = self.remote_tools.selected_tag.clone();
        let remote = self.remote_tools.selected_remote.clone();
        if !valid_remote(&repo, &remote) || !valid_tag(&tag) {
            self.notice = "Choose an existing tag and valid remote".into();
            return;
        }
        let Ok(tag_oid) = repo
            .find_reference(&format!("refs/tags/{tag}"))
            .and_then(|r| {
                r.target()
                    .ok_or_else(|| git2::Error::from_str("tag is not a direct reference"))
            })
        else {
            self.notice = "Could not resolve tag object".into();
            return;
        };
        let Some(remote_url) = remote_url(&repo, &remote) else {
            self.notice = "Could not resolve remote URL".into();
            return;
        };
        let (head_ref, head_oid) = head_identity(&repo);
        self.remote_tools.pending_push = Some(PendingPush::Tag {
            repo_key: self.path.as_deref().map(repo_key).unwrap_or_default(),
            head_ref,
            head_oid,
            remote,
            remote_url,
            tag,
            tag_oid: tag_oid.to_string(),
        });
    }
    fn read_lease_oid(&mut self) {
        let Some(repo) = self.repo() else {
            return;
        };
        let remote = self.remote_tools.selected_remote.clone();
        let target = self.remote_tools.lease_target.trim().to_owned();
        if !valid_remote(&repo, &remote) || !valid_branch(&target) {
            self.notice = "Choose a valid remote and branch target".into();
            return;
        }
        let tracking_ref = format!("refs/remotes/{remote}/{target}");
        match repo
            .find_reference(&tracking_ref)
            .and_then(|r| r.peel_to_commit())
        {
            Ok(commit) => self.remote_tools.lease_expected_oid = commit.id().to_string(),
            Err(_) => {
                self.notice =
                    format!("No observed tracking ref {tracking_ref}; fetch this remote first")
            }
        }
    }
    fn prepare_lease_push(&mut self) {
        let Some(repo) = self.repo() else {
            return;
        };
        let remote = self.remote_tools.selected_remote.clone();
        let target = self.remote_tools.lease_target.trim().to_owned();
        let expected = self.remote_tools.lease_expected_oid.trim().to_owned();
        let source = self.remote_tools.selected_branch.clone();
        if !valid_remote(&repo, &remote) || !valid_branch(&target) || !valid_branch(&source) {
            self.notice = "Choose valid remote and branch names".into();
            return;
        }
        let Some(expected_oid) = parse_full_oid(&expected) else {
            self.notice = "Expected OID must be a full object ID".into();
            return;
        };
        let tracking_ref = format!("refs/remotes/{remote}/{target}");
        if repo
            .find_reference(&tracking_ref)
            .and_then(|r| r.peel_to_commit())
            .map(|c| c.id())
            .ok()
            != Some(expected_oid)
        {
            self.notice = format!("Expected OID does not match {tracking_ref}; read it again");
            return;
        }
        let Ok(source_commit) = repo
            .find_branch(&source, BranchType::Local)
            .and_then(|b| b.into_reference().peel_to_commit())
        else {
            self.notice = "Local source branch not found".into();
            return;
        };
        let Some(remote_url) = remote_url(&repo, &remote) else {
            self.notice = "Could not resolve remote URL".into();
            return;
        };
        let (head_ref, head_oid) = head_identity(&repo);
        self.remote_tools.pending_push = Some(PendingPush::Lease {
            repo_key: self.path.as_deref().map(repo_key).unwrap_or_default(),
            head_ref,
            head_oid,
            remote,
            remote_url,
            source_branch: source,
            source_oid: source_commit.id().to_string(),
            target,
            expected_oid: expected,
        });
    }
    fn pending_is_current(
        &self,
        repo: &Repository,
        current_repo_key: &str,
        pending: &PendingPush,
    ) -> bool {
        match pending {
            PendingPush::Lease {
                repo_key: saved_key,
                head_ref,
                head_oid,
                remote,
                remote_url: saved_url,
                source_branch,
                source_oid,
                target,
                expected_oid,
            } => {
                saved_key == current_repo_key
                    && head_identity(repo) == (head_ref.clone(), head_oid.clone())
                    && self.remote_tools.selected_remote == *remote
                    && self.remote_tools.selected_branch == *source_branch
                    && self.remote_tools.lease_target.trim() == target
                    && self.remote_tools.lease_expected_oid.trim() == expected_oid
                    && remote_url(repo, remote).as_deref() == Some(saved_url)
                    && repo
                        .find_branch(source_branch, BranchType::Local)
                        .and_then(|b| b.into_reference().peel_to_commit())
                        .map(|c| c.id().to_string())
                        .ok()
                        .as_deref()
                        == Some(source_oid.as_str())
                    && repo
                        .find_reference(&format!("refs/remotes/{remote}/{target}"))
                        .and_then(|r| r.peel_to_commit())
                        .map(|c| c.id().to_string())
                        .ok()
                        .as_deref()
                        == Some(expected_oid.as_str())
            }
            PendingPush::Tag {
                repo_key: saved_key,
                head_ref,
                head_oid,
                remote,
                remote_url: saved_url,
                tag,
                tag_oid,
            } => {
                saved_key == current_repo_key
                    && head_identity(repo) == (head_ref.clone(), head_oid.clone())
                    && self.remote_tools.selected_remote == *remote
                    && self.remote_tools.selected_tag == *tag
                    && remote_url(repo, remote).as_deref() == Some(saved_url)
                    && repo
                        .find_reference(&format!("refs/tags/{tag}"))
                        .ok()
                        .and_then(|r| r.target())
                        .map(|o| o.to_string())
                        .as_deref()
                        == Some(tag_oid.as_str())
            }
        }
    }
}

fn repo_key(path: &Path) -> String {
    path.canonicalize()
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .into_owned()
}
fn head_identity(repo: &Repository) -> (String, String) {
    let h = repo.head().ok();
    (
        h.as_ref()
            .and_then(|h| h.name())
            .unwrap_or("(detached HEAD)")
            .to_owned(),
        h.and_then(|h| h.target())
            .map(|o| o.to_string())
            .unwrap_or_default(),
    )
}
fn remote_url(repo: &Repository, name: &str) -> Option<String> {
    let remote = repo.find_remote(name).ok()?;
    Some(format!(
        "{}\n{}",
        remote.url()?,
        remote.pushurl().unwrap_or("")
    ))
}
fn remote_names(repo: &Repository) -> Vec<String> {
    repo.remotes()
        .ok()
        .map(|r| r.iter().flatten().map(str::to_owned).collect())
        .unwrap_or_default()
}
fn valid_ref_part(s: &str) -> bool {
    !s.starts_with('-')
        && !s.starts_with('/')
        && !s.ends_with('/')
        && !s.contains("..")
        && !s.contains("@{")
        && !s.contains([' ', '~', '^', ':', '?', '*', '[', '\\'])
        && !s.bytes().any(|b| b < 0x20 || b == 0x7f)
}
fn valid_branch(s: &str) -> bool {
    valid_ref_part(s) && Reference::is_valid_name(&format!("refs/heads/{s}"))
}
fn valid_tag(s: &str) -> bool {
    valid_ref_part(s) && Reference::is_valid_name(&format!("refs/tags/{s}"))
}
fn valid_remote(repo: &Repository, s: &str) -> bool {
    valid_ref_part(s) && repo.find_remote(s).is_ok()
}
fn parse_full_oid(s: &str) -> Option<Oid> {
    let oid = Oid::from_str(s).ok()?;
    (oid.to_string() == s).then_some(oid)
}
fn local_branches(repo: &Repository) -> Vec<String> {
    repo.branches(Some(BranchType::Local))
        .ok()
        .into_iter()
        .flatten()
        .filter_map(|item| item.ok())
        .filter_map(|(b, _)| b.name().ok().flatten().map(str::to_owned))
        .collect()
}
fn remote_branches(repo: &Repository) -> Vec<String> {
    repo.branches(Some(BranchType::Remote))
        .ok()
        .into_iter()
        .flatten()
        .filter_map(|item| item.ok())
        .filter_map(|(b, _)| b.name().ok().flatten().map(str::to_owned))
        .filter(|n| !n.ends_with("/HEAD"))
        .collect()
}

fn branch_push_args(remote: &str, source_oid: &str, target: &str) -> Result<Vec<String>, String> {
    if !valid_ref_part(remote) || !valid_branch(target) || parse_full_oid(source_oid).is_none() {
        return Err("Invalid push remote, branch, or full source OID".into());
    }
    Ok(vec![
        "push".into(),
        "--no-follow-tags".into(),
        remote.into(),
        format!("{source_oid}:refs/heads/{target}"),
    ])
}
fn tracking_args(repo: &Repository, upstream: &str, local: &str) -> Result<Vec<String>, String> {
    if !valid_branch(local) {
        return Err("Invalid local branch name".into());
    }
    if !remote_names(repo).iter().any(|name| {
        upstream
            .strip_prefix(&format!("{name}/"))
            .is_some_and(valid_branch)
    }) || repo.find_branch(upstream, BranchType::Remote).is_err()
    {
        return Err("Choose a valid remote tracking branch".into());
    }
    Ok(vec![
        "branch".into(),
        format!("--set-upstream-to={upstream}"),
        local.into(),
    ])
}
fn lease_push_args(
    remote: &str,
    source_oid: &str,
    target: &str,
    expected: &str,
) -> Result<Vec<String>, String> {
    if !valid_ref_part(remote)
        || !valid_branch(target)
        || parse_full_oid(source_oid).is_none()
        || parse_full_oid(expected).is_none()
    {
        return Err("Invalid remote, target, or full source/expected OID".into());
    }
    let target_ref = format!("refs/heads/{target}");
    Ok(vec![
        "push".into(),
        "--no-follow-tags".into(),
        format!("--force-with-lease={target_ref}:{expected}"),
        remote.into(),
        format!("{source_oid}:{target_ref}"),
    ])
}
fn tag_push_args(remote: &str, tag: &str, tag_oid: &str) -> Result<Vec<String>, String> {
    if !valid_ref_part(remote) || !valid_tag(tag) || parse_full_oid(tag_oid).is_none() {
        return Err("Invalid remote, tag, or full tag object OID".into());
    }
    Ok(vec![
        "push".into(),
        "--no-follow-tags".into(),
        remote.into(),
        format!("{tag_oid}:refs/tags/{tag}"),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, process::Command};
    fn git(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    }
    #[test]
    fn production_argument_builders_validate_and_pin_inputs() {
        let oid = "0123456789012345678901234567890123456789";
        assert!(branch_push_args("--upload-pack=evil", oid, "main").is_err());
        assert!(lease_push_args("origin", "main", "main", oid).is_err());
        assert_eq!(
            lease_push_args("origin", oid, "release", oid).unwrap(),
            vec![
                "push",
                "--no-follow-tags",
                "--force-with-lease=refs/heads/release:0123456789012345678901234567890123456789",
                "origin",
                "0123456789012345678901234567890123456789:refs/heads/release"
            ]
        );
        assert_eq!(
            tag_push_args("origin", "v1", oid).unwrap()[3],
            format!("{oid}:refs/tags/v1")
        );
    }
    #[test]
    fn local_bare_remote_tracking_tag_and_stale_lease_rejection() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let tmp =
            std::env::temp_dir().join(format!("grit-remote-test-{}-{nonce}", std::process::id()));
        fs::create_dir_all(&tmp).unwrap();
        let work = tmp.join("work");
        let bare = tmp.join("remote.git");
        fs::create_dir(&work).unwrap();
        git(&work, &["init", "-b", "main"]);
        git(&work, &["config", "user.name", "Tester"]);
        git(&work, &["config", "user.email", "test@example.invalid"]);
        fs::write(work.join("file"), "one").unwrap();
        git(&work, &["add", "file"]);
        git(&work, &["commit", "-m", "initial"]);
        let initial = git(&work, &["rev-parse", "HEAD"]);
        git(&tmp, &["init", "--bare", bare.to_str().unwrap()]);
        git(&work, &["remote", "add", "origin", bare.to_str().unwrap()]);
        git(&work, &["push", "-u", "origin", "main"]);
        git(&work, &["fetch", "origin"]);
        let repo = Repository::open(&work).unwrap();
        assert_eq!(
            tracking_args(&repo, "origin/main", "main").unwrap(),
            vec!["branch", "--set-upstream-to=origin/main", "main"]
        );
        assert_eq!(remote_branches(&repo), vec!["origin/main"]);
        git(&work, &["branch", "--set-upstream-to=origin/main", "main"]);
        assert_eq!(
            repo.find_branch("main", BranchType::Local)
                .unwrap()
                .upstream()
                .unwrap()
                .name()
                .unwrap()
                .unwrap(),
            "origin/main"
        );
        git(&work, &["tag", "v1"]);
        let tag_oid = git(&work, &["rev-parse", "refs/tags/v1"]);
        let tag_args = tag_push_args("origin", "v1", &tag_oid).unwrap();
        let tag_args_refs: Vec<&str> = tag_args.iter().map(String::as_str).collect();
        git(&work, &tag_args_refs);
        assert_eq!(git(&bare, &["rev-parse", "refs/tags/v1"]), tag_oid);
        fs::write(work.join("file"), "two").unwrap();
        git(&work, &["commit", "-am", "advance"]);
        let new_oid = git(&work, &["rev-parse", "HEAD"]);
        let stale_args = lease_push_args(
            "origin",
            &new_oid,
            "main",
            "0000000000000000000000000000000000000000",
        )
        .unwrap();
        let stale_refs: Vec<&str> = stale_args.iter().map(String::as_str).collect();
        let out = Command::new("git")
            .arg("-C")
            .arg(&work)
            .args(&stale_refs)
            .output()
            .unwrap();
        assert!(!out.status.success());
        assert_eq!(git(&bare, &["rev-parse", "refs/heads/main"]), initial);
        fs::remove_dir_all(tmp).unwrap();
    }
}
