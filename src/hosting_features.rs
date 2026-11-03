use super::*;
use std::{collections::HashMap, process::Command};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Provider {
    GitHub,
    GitLab,
    Gitea,
    Bitbucket,
    Beanstalk,
    Custom,
}

impl Provider {
    const ALL: [Self; 6] = [
        Self::GitHub,
        Self::GitLab,
        Self::Gitea,
        Self::Bitbucket,
        Self::Beanstalk,
        Self::Custom,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::GitHub => "GitHub",
            Self::GitLab => "GitLab",
            Self::Gitea => "Gitea",
            Self::Bitbucket => "Bitbucket Cloud",
            Self::Beanstalk => "Beanstalk",
            Self::Custom => "Custom host",
        }
    }

    fn detect(host: &str) -> Self {
        let host = host.to_ascii_lowercase();
        if host == "github.com" {
            Self::GitHub
        } else if host == "gitlab.com" {
            Self::GitLab
        } else if host == "gitea.com" || host.contains("gitea") {
            Self::Gitea
        } else if host == "bitbucket.org" {
            Self::Bitbucket
        } else if host.ends_with(".beanstalkapp.com") {
            Self::Beanstalk
        } else {
            Self::Custom
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct RemoteAddress {
    scheme: String,
    authority: String,
    host: String,
    repo_path: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct HostingTarget {
    provider: Provider,
    repo_url: String,
    repo_path: String,
}

#[derive(Default)]
pub(super) struct HostingTools {
    selected_remote: String,
    pub(super) provider_override: String,
    base_branch: String,
    pub(super) custom_repository_url: String,
    pub(super) custom_branch_route: String,
    pub(super) custom_commit_route: String,
    pub(super) custom_compare_route: String,
    active_repo: Option<String>,
    repo_settings: HashMap<String, CustomHostSettings>,
}

#[derive(Clone, Default)]
struct CustomHostSettings {
    provider_override: String,
    repository_url: String,
    branch_route: String,
    commit_route: String,
    compare_route: String,
}

impl HostingTools {
    fn current_settings(&self) -> CustomHostSettings {
        let repository_url = validate_web_url(self.custom_repository_url.trim())
            .ok()
            .map(|address| {
                format!(
                    "{}://{}/{}",
                    address.scheme,
                    address.authority,
                    encode_path(&address.repo_path)
                )
            })
            .unwrap_or_default();
        CustomHostSettings {
            provider_override: self.provider_override.clone(),
            repository_url,
            branch_route: self.custom_branch_route.clone(),
            commit_route: self.custom_commit_route.clone(),
            compare_route: self.custom_compare_route.clone(),
        }
    }

    fn store_active(&mut self) {
        if let Some(repo_key) = self.active_repo.clone() {
            self.repo_settings.insert(repo_key, self.current_settings());
        }
    }

    fn activate_repo(&mut self, repo_key: &str) {
        if self.active_repo.as_deref() == Some(repo_key) {
            return;
        }
        self.store_active();
        let settings = self
            .repo_settings
            .get(repo_key)
            .cloned()
            .unwrap_or_default();
        self.provider_override = settings.provider_override;
        self.custom_repository_url = settings.repository_url;
        self.custom_branch_route = settings.branch_route;
        self.custom_commit_route = settings.commit_route;
        self.custom_compare_route = settings.compare_route;
        self.selected_remote.clear();
        self.base_branch.clear();
        self.active_repo = Some(repo_key.to_owned());
    }

    pub(super) fn persistence_strings(&self, repo_key: &str) -> Vec<String> {
        let settings = if self.active_repo.as_deref() == Some(repo_key) {
            self.current_settings()
        } else {
            self.repo_settings
                .get(repo_key)
                .cloned()
                .unwrap_or_default()
        };
        vec![
            settings.provider_override,
            settings.repository_url,
            settings.branch_route,
            settings.commit_route,
            settings.compare_route,
        ]
    }

    pub(super) fn load_persistence_strings(&mut self, repo_key: &str, values: &[String]) {
        let repository_url = values
            .get(1)
            .and_then(|url| {
                validate_web_url(url).ok().map(|address| {
                    format!(
                        "{}://{}/{}",
                        address.scheme,
                        address.authority,
                        encode_path(&address.repo_path)
                    )
                })
            })
            .unwrap_or_default();
        let settings = CustomHostSettings {
            provider_override: values.first().cloned().unwrap_or_default(),
            repository_url,
            branch_route: values.get(2).cloned().unwrap_or_default(),
            commit_route: values.get(3).cloned().unwrap_or_default(),
            compare_route: values.get(4).cloned().unwrap_or_default(),
        };
        self.repo_settings
            .insert(repo_key.to_owned(), settings.clone());
        if self.active_repo.as_deref() == Some(repo_key) {
            self.provider_override = settings.provider_override;
            self.custom_repository_url = settings.repository_url;
            self.custom_branch_route = settings.branch_route;
            self.custom_commit_route = settings.commit_route;
            self.custom_compare_route = settings.compare_route;
        }
    }
}

impl GritApp {
    pub(super) fn hosting_ui(&mut self, ui: &mut egui::Ui) {
        ui.heading("Hosting and code review");
        let Some(repo) = self.repo() else {
            ui.label("Open a repository to view hosting links.");
            return;
        };
        let repo_key = history_features::repo_storage_key(
            self.path
                .as_deref()
                .unwrap_or_else(|| repo.workdir().unwrap_or(repo.path())),
        );
        self.hosting.activate_repo(&repo_key);
        let remote_names = repo
            .remotes()
            .ok()
            .map(|names| {
                names
                    .iter()
                    .flatten()
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if remote_names.is_empty() {
            ui.label("Add a Git remote to build hosting links.");
            return;
        }
        if self.hosting.selected_remote.is_empty()
            || !remote_names.contains(&self.hosting.selected_remote)
        {
            self.hosting.selected_remote = remote_names
                .iter()
                .find(|name| name.as_str() == "origin")
                .cloned()
                .unwrap_or_else(|| remote_names[0].clone());
        }
        ui.horizontal(|ui| {
            ui.label("Remote");
            egui::ComboBox::from_id_salt("hosting_remote")
                .selected_text(&self.hosting.selected_remote)
                .show_ui(ui, |ui| {
                    for name in &remote_names {
                        ui.selectable_value(&mut self.hosting.selected_remote, name.clone(), name);
                    }
                });
            ui.label("Provider");
            egui::ComboBox::from_id_salt("hosting_provider")
                .selected_text(if self.hosting.provider_override.is_empty() {
                    "Auto"
                } else {
                    provider_from_label(&self.hosting.provider_override)
                        .map(Provider::label)
                        .unwrap_or("Auto")
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.hosting.provider_override, String::new(), "Auto");
                    for provider in Provider::ALL {
                        ui.selectable_value(
                            &mut self.hosting.provider_override,
                            provider.label().to_owned(),
                            provider.label(),
                        );
                    }
                });
        });

        let remote = repo
            .find_remote(&self.hosting.selected_remote)
            .ok()
            .and_then(|remote| remote.url().map(str::to_owned));
        let target = remote.as_deref().and_then(|url| {
            build_target(
                url,
                provider_from_label(&self.hosting.provider_override),
                &self.hosting.custom_repository_url,
            )
            .ok()
        });
        if let Some(target) = &target {
            ui.label(format!("Repository: {}", target.repo_url));
            ui.label(format!("Detected provider: {}", target.provider.label()));
        } else if let Some(remote) = remote.as_deref() {
            match build_target(
                remote,
                provider_from_label(&self.hosting.provider_override),
                &self.hosting.custom_repository_url,
            ) {
                Ok(_) => unreachable!(),
                Err(error) => ui.colored_label(Color32::YELLOW, error),
            };
        }

        if self.hosting.provider_override == Provider::Custom.label() {
            ui.label("Custom host: enter the repository web URL and relative route templates. No credentials are stored.");
            ui.add(
                egui::TextEdit::singleline(&mut self.hosting.custom_repository_url)
                    .hint_text("https://host/account/repository"),
            );
            ui.horizontal(|ui| {
                ui.add_sized(
                    [180., 24.],
                    egui::TextEdit::singleline(&mut self.hosting.custom_branch_route)
                        .hint_text("branch/{branch}"),
                );
                ui.add_sized(
                    [180., 24.],
                    egui::TextEdit::singleline(&mut self.hosting.custom_commit_route)
                        .hint_text("commit/{commit}"),
                );
                ui.add_sized(
                    [200., 24.],
                    egui::TextEdit::singleline(&mut self.hosting.custom_compare_route)
                        .hint_text("compare/{base}...{head}"),
                );
            });
        }
        self.hosting.store_active();

        let busy = self.operation.is_some();
        let current_branch = self.branch.clone();
        let current_commit = self.selected_commit.clone().or_else(|| {
            repo.head()
                .ok()
                .and_then(|head| head.peel_to_commit().ok())
                .map(|commit| commit.id().to_string())
        });
        if let Some(target) = target {
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(true, egui::Button::new("Open repository"))
                    .clicked()
                {
                    open_hosting_url(&target.repo_url, &mut self.notice);
                }
                if current_branch != "(detached HEAD)" && !current_branch.is_empty() {
                    match source_branch_and_remote(&repo, &current_branch) {
                        Some((branch, _, source_url)) => {
                            let source_target = build_target(
                                &source_url,
                                provider_from_label(&self.hosting.provider_override),
                                &self.hosting.custom_repository_url,
                            );
                            match source_target.and_then(|source_target| {
                                branch_url(&source_target, &branch, &self.hosting)
                            }) {
                                Ok(Some(url)) => {
                                    if ui
                                        .add_enabled(
                                            true,
                                            egui::Button::new("Open upstream branch"),
                                        )
                                        .clicked()
                                    {
                                        open_hosting_url(&url, &mut self.notice);
                                    }
                                }
                                Ok(None) => {
                                    ui.add_enabled(
                                        false,
                                        egui::Button::new("Branch link unavailable"),
                                    );
                                }
                                Err(error) => {
                                    ui.label(error);
                                }
                            }
                        }
                        None => {
                            ui.add_enabled(false, egui::Button::new("Branch has no upstream"));
                        }
                    }
                }
                if let Some(commit) = current_commit.as_deref() {
                    match commit_url(&target, commit, &self.hosting) {
                        Ok(Some(url)) => {
                            if ui
                                .add_enabled(true, egui::Button::new("Open selected commit"))
                                .clicked()
                            {
                                open_hosting_url(&url, &mut self.notice);
                            }
                        }
                        Ok(None) => {
                            ui.add_enabled(false, egui::Button::new("Commit link unavailable"));
                        }
                        Err(error) => {
                            ui.label(error);
                        }
                    }
                }
            });

            let base_branches = remote_branches_for(&repo, &self.hosting.selected_remote);
            if self.hosting.base_branch.is_empty()
                || !base_branches.contains(&self.hosting.base_branch)
            {
                self.hosting.base_branch = repo
                    .find_reference(&format!(
                        "refs/remotes/{}/HEAD",
                        self.hosting.selected_remote
                    ))
                    .ok()
                    .and_then(|reference| reference.resolve().ok())
                    .and_then(|reference| reference.shorthand().map(str::to_owned))
                    .and_then(|name| {
                        name.strip_prefix(&format!("{}/", self.hosting.selected_remote))
                            .map(str::to_owned)
                    })
                    .or_else(|| base_branches.first().cloned())
                    .unwrap_or_default();
            }
            ui.horizontal(|ui| {
                ui.label("Base branch");
                egui::ComboBox::from_id_salt("hosting_base_branch")
                    .selected_text(if self.hosting.base_branch.is_empty() {
                        "Select base branch"
                    } else {
                        &self.hosting.base_branch
                    })
                    .show_ui(ui, |ui| {
                        for branch in &base_branches {
                            ui.selectable_value(
                                &mut self.hosting.base_branch,
                                branch.clone(),
                                branch,
                            );
                        }
                    });
                if base_branches.is_empty() {
                    ui.add_sized(
                        [150., 24.],
                        egui::TextEdit::singleline(&mut self.hosting.base_branch)
                            .hint_text("Base branch"),
                    );
                }
            });
            let source = source_branch_and_remote(&repo, &current_branch).or_else(|| {
                Some((
                    current_branch.clone(),
                    self.hosting.selected_remote.clone(),
                    remote.clone().unwrap_or_default(),
                ))
            });
            let can_review = current_branch != "(detached HEAD)"
                && !current_branch.is_empty()
                && !self.hosting.base_branch.is_empty();
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        !busy && can_review,
                        egui::Button::new("Open new PR / MR / review"),
                    )
                    .clicked()
                    && let Some((head_branch, _source_remote, source_url)) = source.as_ref()
                {
                    let source_target = if target.provider == Provider::Custom {
                        Ok(target.clone())
                    } else {
                        build_target(
                            source_url,
                            provider_from_label(&self.hosting.provider_override),
                            &self.hosting.custom_repository_url,
                        )
                    };
                    match source_target.and_then(|source_target| {
                        compare_url(
                            &target,
                            &self.hosting.base_branch,
                            head_branch,
                            &source_target,
                            &self.hosting,
                        )
                    }) {
                        Ok(Some(url)) => open_hosting_url(&url, &mut self.notice),
                        Ok(None) => open_hosting_url(&target.repo_url, &mut self.notice),
                        Err(error) => self.notice = error,
                    }
                }
            });
            if target.provider == Provider::Beanstalk {
                ui.label("Beanstalk requires selecting Reviews and choosing source/base branches in its repository UI; no stable deep link can be derived from the Git remote URL.");
            } else {
                ui.label("This opens the provider's browser form. Grit does not create or submit a PR/MR and does not send API requests.");
                if source_branch_and_remote(&repo, &current_branch).is_none() {
                    ui.label("Push this branch to a remote before submitting the browser form; no upstream source repository is configured.");
                }
            }
        }
    }
}

fn provider_from_label(label: &str) -> Option<Provider> {
    Provider::ALL
        .into_iter()
        .find(|provider| provider.label() == label)
}

fn clean_remote_path(path: &str) -> Result<String, String> {
    if path.contains(['?', '#']) {
        return Err("Remote URL path cannot contain a query or fragment".into());
    }
    let path = path
        .trim_matches('/')
        .split(['?', '#'])
        .next()
        .unwrap_or_default();
    let path = path.strip_suffix(".git").unwrap_or(path);
    if path.is_empty()
        || path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
        || path
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
    {
        return Err("Remote URL does not contain a safe repository path".into());
    }
    Ok(path.to_owned())
}

fn parse_remote_address(remote: &str) -> Result<RemoteAddress, String> {
    if remote.is_empty()
        || remote
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
        || (remote.as_bytes().get(1) == Some(&b':')
            && remote
                .as_bytes()
                .get(2)
                .is_some_and(|byte| matches!(*byte, b'/' | b'\\')))
    {
        return Err("Remote URL is empty or contains whitespace/control characters".into());
    }
    if let Some((scheme, rest)) = remote.split_once("://") {
        if !matches!(scheme, "https" | "http" | "ssh" | "git") {
            return Err("Unsupported remote URL scheme".into());
        }
        let (authority, path) = rest
            .split_once('/')
            .ok_or("Remote URL has no repository path")?;
        let authority = authority.rsplit('@').next().unwrap_or(authority);
        if authority.is_empty() {
            return Err("Remote URL has no host".into());
        }
        let host_port = authority
            .rsplit_once(':')
            .filter(|(host, port)| !host.contains(']') && port.chars().all(|c| c.is_ascii_digit()))
            .map(|(host, _)| host)
            .unwrap_or(authority);
        let host = host_port.trim_matches(['[', ']']).to_ascii_lowercase();
        let scheme = match scheme {
            "ssh" | "git" => "https",
            other => other,
        }
        .to_owned();
        let authority = if scheme == "https"
            && (remote.starts_with("ssh://") || remote.starts_with("git://"))
        {
            host.clone()
        } else {
            authority.to_owned()
        };
        return Ok(RemoteAddress {
            scheme,
            authority,
            host,
            repo_path: clean_remote_path(path)?,
        });
    }
    if let Some((authority, path)) = remote.split_once(':')
        && (authority.contains('@') || !authority.contains('/'))
    {
        let host = authority
            .rsplit('@')
            .next()
            .unwrap_or(authority)
            .to_ascii_lowercase();
        if host.is_empty() {
            return Err("Remote URL has no host".into());
        }
        return Ok(RemoteAddress {
            scheme: "https".into(),
            authority: host.clone(),
            host,
            repo_path: clean_remote_path(path)?,
        });
    }
    Err("Unsupported remote URL; use HTTPS, SSH, or scp-style Git remotes".into())
}

fn validate_web_url(url: &str) -> Result<RemoteAddress, String> {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err("Web URL must start with HTTP or HTTPS".into());
    }
    if url.contains(['?', '#']) {
        return Err("Web repository URL cannot contain a query or fragment".into());
    }
    let address = parse_remote_address(url)?;
    if !matches!(address.scheme.as_str(), "https" | "http") {
        return Err("Web URL must use HTTP or HTTPS".into());
    }
    if url.contains('@') {
        return Err("Web repository URL cannot contain credentials".into());
    }
    Ok(address)
}

fn encode_component(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

fn encode_path(value: &str) -> String {
    value
        .split('/')
        .map(encode_path_segment)
        .collect::<Vec<_>>()
        .join("/")
}

fn encode_path_segment(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut encoded = String::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && bytes[index + 1].is_ascii_hexdigit()
            && bytes[index + 2].is_ascii_hexdigit()
        {
            encoded.push('%');
            encoded.push((bytes[index + 1] as char).to_ascii_uppercase());
            encoded.push((bytes[index + 2] as char).to_ascii_uppercase());
            index += 3;
        } else {
            let character = value[index..]
                .chars()
                .next()
                .expect("index is on a character boundary");
            encoded.push_str(&encode_component(&character.to_string()));
            index += character.len_utf8();
        }
    }
    encoded
}

fn build_target(
    remote: &str,
    override_provider: Option<Provider>,
    custom_url: &str,
) -> Result<HostingTarget, String> {
    let mut address = parse_remote_address(remote)?;
    let detected = Provider::detect(&address.host);
    let provider = override_provider.unwrap_or(detected);
    if provider == Provider::Custom {
        address = validate_web_url(custom_url.trim())?;
    }
    let (repo_path, repo_url) = if provider == Provider::Beanstalk {
        let web_host = address
            .host
            .replace(".git.beanstalkapp.com", ".beanstalkapp.com");
        (address.repo_path.clone(), format!("https://{web_host}"))
    } else {
        let path = encode_path(&address.repo_path);
        let repo_url = format!("{}://{}/{}", address.scheme, address.authority, path);
        (address.repo_path, repo_url)
    };
    Ok(HostingTarget {
        provider,
        repo_url,
        repo_path,
    })
}

fn owner(target: &HostingTarget) -> &str {
    target.repo_path.split('/').next().unwrap_or_default()
}

fn branch_url(
    target: &HostingTarget,
    branch: &str,
    tools: &HostingTools,
) -> Result<Option<String>, String> {
    let branch = encode_component(branch);
    let suffix = match target.provider {
        Provider::GitHub => format!("tree/{branch}"),
        Provider::GitLab => format!("-/tree/{branch}"),
        Provider::Gitea => format!("src/branch/{branch}"),
        Provider::Bitbucket => format!("branch/{branch}"),
        Provider::Beanstalk => return Ok(None),
        Provider::Custom => {
            render_custom_route(&tools.custom_branch_route, &[("branch", &branch)])?
        }
    };
    Ok(Some(append_repo_route(&target.repo_url, &suffix)))
}

fn commit_url(
    target: &HostingTarget,
    commit: &str,
    tools: &HostingTools,
) -> Result<Option<String>, String> {
    if commit.is_empty() || !commit.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("Commit ID must contain only hexadecimal characters".into());
    }
    let commit = encode_component(commit);
    let suffix = match target.provider {
        Provider::GitHub => format!("commit/{commit}"),
        Provider::GitLab => format!("-/commit/{commit}"),
        Provider::Gitea => format!("commit/{commit}"),
        Provider::Bitbucket => format!("commits/{commit}"),
        Provider::Beanstalk => return Ok(None),
        Provider::Custom => {
            render_custom_route(&tools.custom_commit_route, &[("commit", &commit)])?
        }
    };
    Ok(Some(append_repo_route(&target.repo_url, &suffix)))
}

fn compare_url(
    target: &HostingTarget,
    base: &str,
    head: &str,
    source: &HostingTarget,
    tools: &HostingTools,
) -> Result<Option<String>, String> {
    if base.is_empty() || head.is_empty() {
        return Err("Choose both a base and source branch".into());
    }
    let head_spec = if target.repo_path == source.repo_path {
        encode_component(head)
    } else {
        format!(
            "{}:{}",
            encode_component(owner(source)),
            encode_component(head)
        )
    };
    let base = encode_component(base);
    let suffix = match target.provider {
        Provider::GitHub => format!("compare/{base}...{head_spec}?quick_pull=1"),
        Provider::GitLab => format!(
            "merge_requests/new?merge_request%5Bsource_branch%5D={}&merge_request%5Btarget_branch%5D={base}",
            encode_query_value(head)
        ),
        Provider::Gitea => format!("compare/{base}...{head_spec}"),
        Provider::Bitbucket => format!(
            "pull-requests/new?source={}&dest={base}&t=1",
            encode_query_value(head)
        ),
        Provider::Beanstalk => return Ok(None),
        Provider::Custom => render_custom_route(
            &tools.custom_compare_route,
            &[("base", &base), ("head", &head_spec)],
        )?,
    };
    Ok(Some(append_repo_route(&target.repo_url, &suffix)))
}

fn encode_query_value(value: &str) -> String {
    encode_component(value)
}

fn append_repo_route(repo_url: &str, route: &str) -> String {
    let (path, query) = route.split_once('?').unwrap_or((route, ""));
    let mut result = format!(
        "{}/{}",
        repo_url.trim_end_matches('/'),
        path.trim_start_matches('/')
    );
    if !query.is_empty() {
        result.push('?');
        result.push_str(query);
    }
    result
}

fn render_custom_route(template: &str, values: &[(&str, &str)]) -> Result<String, String> {
    if template.trim().is_empty() {
        return Err("Configure a custom-host route template".into());
    }
    if template.contains(['\n', '\r', '\0', '?', '#'])
        || template.contains("://")
        || template.starts_with("//")
        || template.split('/').any(|part| part == "..")
    {
        return Err(
            "Custom route must be a relative path without query/fragment or parent traversal"
                .into(),
        );
    }
    let mut output = template.to_owned();
    for (key, value) in values {
        output = output.replace(&format!("{{{key}}}"), value);
    }
    if output.contains('{') || output.contains('}') {
        return Err("Custom route contains an unknown placeholder".into());
    }
    Ok(output.trim_start_matches('/').to_owned())
}

fn remote_branches_for(repo: &Repository, remote: &str) -> Vec<String> {
    repo.branches(Some(BranchType::Remote))
        .ok()
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter_map(|(branch, _)| branch.name().ok().flatten().map(str::to_owned))
        .filter_map(|name| name.strip_prefix(&format!("{remote}/")).map(str::to_owned))
        .collect()
}

fn source_branch_and_remote(
    repo: &Repository,
    local_branch: &str,
) -> Option<(String, String, String)> {
    if local_branch.is_empty() || local_branch == "(detached HEAD)" {
        return None;
    }
    let local = repo.find_branch(local_branch, BranchType::Local).ok()?;
    let upstream = local.upstream().ok()?;
    let upstream_name = upstream
        .name()
        .ok()
        .flatten()?
        .strip_prefix("refs/remotes/")?
        .to_owned();
    let remotes = repo.remotes().ok()?;
    let mut best: Option<(String, String, String)> = None;
    for remote in remotes.iter().flatten() {
        let prefix = format!("{remote}/");
        if let Some(branch) = upstream_name.strip_prefix(&prefix)
            && best
                .as_ref()
                .is_none_or(|(best_name, _, _)| remote.len() > best_name.len())
        {
            let url = repo.find_remote(remote).ok()?.url()?.to_owned();
            best = Some((remote.to_owned(), branch.to_owned(), url));
        }
    }
    let (remote, branch, url) = best?;
    Some((branch, remote, url))
}

fn open_hosting_url(url: &str, notice: &mut String) {
    if validate_http_url(url).is_err() {
        *notice = "Refusing to open an invalid hosting URL".into();
        return;
    }
    let mut command = browser_command(url);
    match command.spawn() {
        Ok(_) => *notice = format!("Opened {url}"),
        Err(error) => *notice = format!("Could not open browser: {error}"),
    }
}

fn validate_http_url(url: &str) -> Result<(), String> {
    if !(url.starts_with("https://") || url.starts_with("http://"))
        || url
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
    {
        return Err("Hosting URL must be a clean HTTP(S) URL".into());
    }
    Ok(())
}

fn browser_command(url: &str) -> Command {
    #[cfg(target_os = "windows")]
    {
        let mut command = Command::new("rundll32.exe");
        command.args(["url.dll,FileProtocolHandler", url]);
        command
    }
    #[cfg(target_os = "macos")]
    {
        let mut command = Command::new("open");
        command.arg(url);
        command
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let mut command = Command::new("xdg-open");
        command.arg(url);
        command
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tools() -> HostingTools {
        HostingTools {
            custom_branch_route: "branches/{branch}".into(),
            custom_commit_route: "changesets/{commit}".into(),
            custom_compare_route: "review/{base}...{head}".into(),
            ..HostingTools::default()
        }
    }

    #[test]
    fn parses_supported_remote_url_shapes_without_exposing_credentials() {
        let github = build_target("git@github.com:octo-org/repo.git", None, "").unwrap();
        assert_eq!(github.provider, Provider::GitHub);
        assert_eq!(github.repo_url, "https://github.com/octo-org/repo");
        let gitlab =
            build_target("ssh://git@gitlab.com/group/subgroup/repo.git", None, "").unwrap();
        assert_eq!(gitlab.repo_url, "https://gitlab.com/group/subgroup/repo");
        let bitbucket = build_target(
            "https://user:secret@bitbucket.org/workspace/repo.git",
            None,
            "",
        )
        .unwrap();
        assert_eq!(bitbucket.provider, Provider::Bitbucket);
        assert!(!bitbucket.repo_url.contains("secret"));
        let beanstalk = build_target("git@acme.git.beanstalkapp.com:/repo.git", None, "").unwrap();
        assert_eq!(beanstalk.repo_url, "https://acme.beanstalkapp.com");
        assert_eq!(beanstalk.provider, Provider::Beanstalk);
        let ssh_port = build_target(
            "ssh://git@git.example.org:2222/team/repo.git",
            Some(Provider::Gitea),
            "",
        )
        .unwrap();
        assert_eq!(ssh_port.repo_url, "https://git.example.org/team/repo");
    }

    #[test]
    fn creates_correct_provider_branch_commit_and_new_review_urls_with_escaping() {
        let tools = tools();
        let github = build_target("git@github.com:octo/repo.git", None, "").unwrap();
        assert_eq!(
            branch_url(&github, "feature/a b", &tools).unwrap().unwrap(),
            "https://github.com/octo/repo/tree/feature%2Fa%20b"
        );
        assert_eq!(
            commit_url(&github, "abc1234", &tools).unwrap().unwrap(),
            "https://github.com/octo/repo/commit/abc1234"
        );
        assert_eq!(
            compare_url(&github, "main", "feature/a b", &github, &tools)
                .unwrap()
                .unwrap(),
            "https://github.com/octo/repo/compare/main...feature%2Fa%20b?quick_pull=1"
        );
        let fork = build_target("git@github.com:contrib/repo.git", None, "").unwrap();
        assert_eq!(
            compare_url(&github, "release/1", "topic", &fork, &tools)
                .unwrap()
                .unwrap(),
            "https://github.com/octo/repo/compare/release%2F1...contrib:topic?quick_pull=1"
        );

        let gitlab = build_target("git@gitlab.com:group/project.git", None, "").unwrap();
        assert_eq!(
            compare_url(&gitlab, "main", "topic/one", &gitlab, &tools)
                .unwrap()
                .unwrap(),
            "https://gitlab.com/group/project/merge_requests/new?merge_request%5Bsource_branch%5D=topic%2Fone&merge_request%5Btarget_branch%5D=main"
        );
        let bitbucket = build_target("git@bitbucket.org:workspace/repo.git", None, "").unwrap();
        assert_eq!(
            compare_url(&bitbucket, "main", "feature/x", &bitbucket, &tools)
                .unwrap()
                .unwrap(),
            "https://bitbucket.org/workspace/repo/pull-requests/new?source=feature%2Fx&dest=main&t=1"
        );
        let gitea = build_target(
            "ssh://git@gitea.example.org/team/repo.git",
            Some(Provider::Gitea),
            "",
        )
        .unwrap();
        assert_eq!(
            compare_url(&gitea, "main", "fix", &gitea, &tools)
                .unwrap()
                .unwrap(),
            "https://gitea.example.org/team/repo/compare/main...fix"
        );
    }

    #[test]
    fn custom_host_requires_explicit_safe_web_url_and_route_templates() {
        assert!(build_target("git@git.example:team/repo.git", Some(Provider::Custom), "").is_err());
        let target = build_target(
            "git@git.example:team/repo.git",
            Some(Provider::Custom),
            "https://git.example/team/repo",
        )
        .unwrap();
        let tools = tools();
        assert_eq!(
            branch_url(&target, "feature/a", &tools).unwrap().unwrap(),
            "https://git.example/team/repo/branches/feature%2Fa"
        );
        assert!(render_custom_route("https://evil.invalid/{branch}", &[("branch", "x")]).is_err());
        assert!(render_custom_route("../{branch}", &[("branch", "x")]).is_err());
    }

    #[test]
    fn rejects_untrusted_schemes_and_query_or_fragment_in_custom_repository_url() {
        assert!(validate_web_url("javascript:alert(1)").is_err());
        assert!(validate_web_url("https://user:pass@git.example/team/repo").is_err());
        assert!(validate_http_url("file:///etc/passwd").is_err());
    }

    #[test]
    fn credential_bearing_custom_urls_are_never_persisted_or_displayed() {
        let mut tools = HostingTools {
            active_repo: Some("repo-key".into()),
            custom_repository_url: "https://alice:secret-token@git.example/team/repo".into(),
            ..HostingTools::default()
        };
        assert!(
            tools
                .persistence_strings("repo-key")
                .iter()
                .all(|value| !value.contains("secret-token"))
        );
        tools.load_persistence_strings(
            "other-repo",
            &[
                "Custom host".into(),
                "https://alice:secret-token@git.example/team/repo".into(),
                "branch/{branch}".into(),
                "commit/{commit}".into(),
                "compare/{base}...{head}".into(),
            ],
        );
        assert!(
            tools
                .persistence_strings("other-repo")
                .iter()
                .all(|value| !value.contains("secret-token"))
        );
        assert!(tools.repo_settings["other-repo"].repository_url.is_empty());
    }
}
