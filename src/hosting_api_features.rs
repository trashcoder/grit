//! Native pull-request and code-review APIs. Credentials live only in memory and
//! are passed to curl through its stdin config, never through argv or diagnostics.
use super::*;
use std::{
    io::Read,
    process::{Command, Stdio},
    sync::mpsc::{self, Receiver},
};

const MAX_RESPONSE: usize = 2 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HostingProvider {
    GitHub,
    GitLab,
    Gitea,
    Bitbucket,
    Beanstalk,
}
impl HostingProvider {
    const ALL: [Self; 5] = [
        Self::GitHub,
        Self::GitLab,
        Self::Gitea,
        Self::Bitbucket,
        Self::Beanstalk,
    ];
    fn label(self) -> &'static str {
        match self {
            Self::GitHub => "GitHub",
            Self::GitLab => "GitLab",
            Self::Gitea => "Gitea",
            Self::Bitbucket => "Bitbucket Cloud",
            Self::Beanstalk => "Beanstalk",
        }
    }
    fn api_prefix(self, base: &str) -> String {
        match self {
            Self::GitHub if base.trim_end_matches('/') == "https://api.github.com" => {
                base.trim_end_matches('/').to_owned()
            }
            Self::GitHub if base.trim_end_matches('/') == "https://github.com" => {
                "https://api.github.com".into()
            }
            Self::GitHub => format!("{}/api/v3", base.trim_end_matches('/')),
            Self::GitLab => format!("{}/api/v4", base.trim_end_matches('/')),
            Self::Gitea => format!("{}/api/v1", base.trim_end_matches('/')),
            Self::Bitbucket => "https://api.bitbucket.org/2.0".into(),
            Self::Beanstalk => base.trim_end_matches('/').to_owned(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ReviewItem {
    pub id: String,
    pub title: String,
    pub source: String,
    pub target: String,
    pub state: String,
    pub body: String,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct HostRepository {
    id: String,
    name: String,
    full_name: String,
    https: String,
    ssh: String,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct ApiConfig {
    provider: usize,
    base: String,
    owner: String,
    repo: String,
    username: String,
}
#[derive(Default)]
pub(crate) struct HostingApiTools {
    accounts: Vec<(ApiConfig, String)>,
    active_account: usize,
    account_repository_paths: Vec<HashMap<String, String>>,
    config: ApiConfig,
    token: String,
    source: String,
    target: String,
    title: String,
    body: String,
    repository_search: String,
    repositories: Vec<HostRepository>,
    repositories_page: usize,
    reviews_page: usize,
    has_more_repositories: bool,
    has_more_reviews: bool,
    selected_repository_key: Option<String>,
    selected_local_path: String,
    local_path_valid: bool,
    clone_ssh: bool,
    open_request: Option<PathBuf>,
    clone_request: Option<(String, String)>,
    reviews: Vec<ReviewItem>,
    selected: Option<usize>,
    detail: Option<ReviewItem>,
    status: String,
    pending: Option<Receiver<Result<ApiResult, String>>>,
    confirm_create: bool,
}
struct ApiResult {
    reviews: Vec<ReviewItem>,
    detail: Option<ReviewItem>,
    message: String,
    repositories: Vec<HostRepository>,
    append_reviews: bool,
    append_repositories: bool,
}
impl HostingApiTools {
    /// Persist only non-secret metadata. The token is deliberately omitted.
    pub(crate) fn persistence_strings(&self) -> Vec<String> {
        let mut accounts = self.accounts.clone();
        if accounts.is_empty() {
            accounts.push((self.config.clone(), String::new()));
        } else if self.active_account < accounts.len() {
            accounts[self.active_account] = (self.config.clone(), String::new());
        }
        let records: Vec<_> = accounts
            .iter()
            .map(|(c, _)| {
                serde_json::json!([
                    c.provider,
                    if HostingProvider::ALL
                        .get(c.provider)
                        .is_some_and(|p| valid_base(&c.base, *p))
                    {
                        c.base.clone()
                    } else {
                        String::new()
                    },
                    c.owner,
                    c.repo,
                    c.username
                ])
            })
            .collect();
        let mut repository_paths = self.account_repository_paths.clone();
        repository_paths.resize_with(accounts.len(), HashMap::new);
        repository_paths.truncate(accounts.len());
        vec![serde_json::json!({"active":self.active_account,"accounts":records,"repository_paths":repository_paths}).to_string()]
    }
    pub(crate) fn load_persistence_strings(&mut self, values: &[String]) {
        if let Some(raw) = values
            .first()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
            .filter(|v| v.get("accounts").is_some())
        {
            self.accounts = raw
                .get("accounts")
                .and_then(|x| x.as_array())
                .into_iter()
                .flatten()
                .map(|entry| {
                    let a = entry.as_array();
                    let strings: Vec<String> = (0..5)
                        .map(|i| {
                            a.and_then(|v| v.get(i))
                                .map(|v| {
                                    v.as_str()
                                        .map(str::to_owned)
                                        .unwrap_or_else(|| v.to_string())
                                })
                                .unwrap_or_default()
                        })
                        .collect();
                    (config_from_strings(&strings), String::new())
                })
                .collect();
            self.account_repository_paths = raw
                .get("repository_paths")
                .and_then(|x| x.as_array())
                .into_iter()
                .flatten()
                .map(|entry| {
                    entry
                        .as_object()
                        .map(|o| {
                            o.iter()
                                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_owned())))
                                .collect()
                        })
                        .unwrap_or_default()
                })
                .collect();
            self.account_repository_paths
                .resize_with(self.accounts.len(), HashMap::new);
            self.account_repository_paths.truncate(self.accounts.len());
            self.active_account = raw.get("active").and_then(|x| x.as_u64()).unwrap_or(0) as usize;
            if self.active_account >= self.accounts.len() {
                self.active_account = 0;
            }
            if let Some((c, t)) = self.accounts.get(self.active_account).cloned() {
                self.config = c;
                self.token = t;
            }
            self.clear_remote_data();
            return;
        }
        self.config.provider = values
            .first()
            .and_then(|v| v.parse::<usize>().ok())
            .filter(|v| *v < HostingProvider::ALL.len())
            .unwrap_or(0);
        self.config.base = values.get(1).cloned().unwrap_or_default();
        self.config.owner = values.get(2).cloned().unwrap_or_default();
        self.config.repo = values.get(3).cloned().unwrap_or_default();
        self.config.username = values.get(4).cloned().unwrap_or_default();
        self.token.clear();
        self.accounts = vec![(self.config.clone(), String::new())];
        self.active_account = 0;
        self.account_repository_paths = vec![HashMap::new()];
    }
    pub(crate) fn hosting_api_ui(&mut self, ui: &mut egui::Ui) {
        self.ensure_account();
        ui.heading("Hosting accounts and reviews");
        let previous_account = self.active_account;
        ui.horizontal(|ui| {
            let mut selected = false;
            ui.add_enabled_ui(self.pending.is_none(), |ui| {
                selected = egui::ComboBox::from_id_salt("hosting_api_account")
                    .selected_text(format!(
                        "{} · {}",
                        HostingProvider::ALL[self.config.provider].label(),
                        if self.config.username.is_empty() {
                            self.config.base.clone()
                        } else {
                            self.config.username.clone()
                        }
                    ))
                    .show_ui(ui, |ui| {
                        for (i, (c, _)) in self.accounts.iter().enumerate() {
                            ui.selectable_value(
                                &mut self.active_account,
                                i,
                                format!(
                                    "{} · {}",
                                    HostingProvider::ALL[c.provider].label(),
                                    if c.username.is_empty() {
                                        c.base.clone()
                                    } else {
                                        c.username.clone()
                                    }
                                ),
                            );
                        }
                    })
                    .response
                    .changed();
            });
            if selected {
                self.save_account(previous_account);
                self.load_active();
                self.clear_remote_data();
            }
            if ui
                .add_enabled(self.pending.is_none(), egui::Button::new("Add account"))
                .clicked()
            {
                self.save_active();
                self.accounts.push((ApiConfig::default(), String::new()));
                self.account_repository_paths.push(HashMap::new());
                self.active_account = self.accounts.len() - 1;
                self.load_active();
                self.clear_remote_data();
            }
            if self.accounts.len() > 1
                && ui
                    .add_enabled(self.pending.is_none(), egui::Button::new("Remove account"))
                    .clicked()
            {
                self.accounts.remove(self.active_account);
                if self.active_account < self.account_repository_paths.len() {
                    self.account_repository_paths.remove(self.active_account);
                }
                self.active_account = self.active_account.min(self.accounts.len() - 1);
                self.load_active();
                self.clear_remote_data();
            }
        });
        let previous_config = self.config.clone();
        ui.add_enabled_ui(self.pending.is_none(),|ui|{
        egui::ComboBox::from_id_salt("hosting_api_provider")
            .selected_text(HostingProvider::ALL[self.config.provider].label())
            .show_ui(ui, |ui| {
                for (i, p) in HostingProvider::ALL.iter().enumerate() {
                    ui.selectable_value(&mut self.config.provider, i, p.label());
                }
            });
        ui.horizontal(|ui| {
            ui.label("HTTPS API base");
            ui.add(
                egui::TextEdit::singleline(&mut self.config.base)
                    .hint_text("https://github.com (or https://gitlab.example)"),
            );
        });
        ui.columns(2, |cols| {
            cols[0].label("Owner / workspace");
            let width = cols[0].available_width();
            cols[0].add(egui::TextEdit::singleline(&mut self.config.owner).desired_width(width));
            cols[1].label("Repository");
            let width = cols[1].available_width();
            cols[1].add(egui::TextEdit::singleline(&mut self.config.repo).desired_width(width));
        });
        ui.columns(2, |cols| {
            cols[0].label("Username (when required)");
            let width = cols[0].available_width();
            cols[0].add(egui::TextEdit::singleline(&mut self.config.username).desired_width(width));
            cols[1].label("Session token");
            let width = cols[1].available_width();
            cols[1].add(egui::TextEdit::singleline(&mut self.token).password(true).desired_width(width));
        });
        ui.label("Token is memory-only and cleared when the app closes. No API request runs until you choose an action.");
        });
        if self.config != previous_config {
            let repository_context_changed = self.config.provider != previous_config.provider
                || self.config.base != previous_config.base
                || self.config.username != previous_config.username;
            self.clear_reviews();
            if repository_context_changed {
                self.repositories.clear();
                self.has_more_repositories = false;
                self.repositories_page = 1;
                self.selected_repository_key = None;
                self.selected_local_path.clear();
                self.local_path_valid = false;
            }
            self.save_active();
        }
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    self.pending.is_none(),
                    egui::Button::new("Load repositories"),
                )
                .clicked()
            {
                self.repositories_page = 1;
                self.repositories.clear();
                self.request(ApiAction::Repositories { page: 1 });
            }
            if ui
                .add_enabled(self.pending.is_none(), egui::Button::new("Load reviews"))
                .clicked()
            {
                self.reviews_page = 1;
                self.clear_reviews();
                self.request(ApiAction::List { page: 1 });
            }
            if ui
                .add_enabled(self.pending.is_none(), egui::Button::new("Create review…"))
                .clicked()
            {
                self.confirm_create = true;
            }
        });
        if !self.repositories.is_empty() {
            ui.horizontal(|ui| {
                ui.label("Filter repositories");
                ui.text_edit_singleline(&mut self.repository_search);
            });
            let repository_search = self.repository_search.to_lowercase();
            for (i, r) in self
                .repositories
                .clone()
                .iter()
                .enumerate()
                .filter(|(_, r)| {
                    repository_search.is_empty()
                        || r.full_name.to_lowercase().contains(&repository_search)
                })
            {
                if ui
                    .add_enabled_ui(self.pending.is_none(), |ui| {
                        ui.selectable_label(false, format!("{}  ·  {}", r.full_name, r.https))
                    })
                    .inner
                    .clicked()
                {
                    self.config.owner = if self.config.provider == 4 {
                        String::new()
                    } else {
                        r.full_name
                            .rsplit_once('/')
                            .map(|p| p.0.to_owned())
                            .unwrap_or_default()
                    };
                    self.config.repo = if self.config.provider == 4 {
                        r.id.clone()
                    } else {
                        r.full_name
                            .rsplit_once('/')
                            .map(|p| p.1.to_owned())
                            .unwrap_or_else(|| r.name.clone())
                    };
                    self.selected = Some(i);
                    self.selected_repository_key =
                        Some(remote_repository_key(self.config.provider, r));
                    self.selected_local_path = self
                        .account_repository_paths
                        .get(self.active_account)
                        .and_then(|m| {
                            m.get(self.selected_repository_key.as_deref().unwrap_or_default())
                        })
                        .cloned()
                        .unwrap_or_default();
                    self.local_path_valid =
                        validate_local_repository(Path::new(&self.selected_local_path)).is_ok();
                    self.clear_reviews();
                    self.save_active();
                }
            }
            if self.has_more_repositories
                && ui
                    .add_enabled(
                        self.pending.is_none(),
                        egui::Button::new("Load next repository page"),
                    )
                    .clicked()
            {
                self.repositories_page += 1;
                self.request(ApiAction::Repositories {
                    page: self.repositories_page,
                });
            }
        }
        self.selected_repository_ui(ui);
        if self.confirm_create {
            ui.separator();
            ui.label("Review the exact source and target branches, title, and description before creating the remote request.");
            ui.horizontal(|ui| {
                ui.label("Source");
                ui.text_edit_singleline(&mut self.source);
                ui.label("Target");
                ui.text_edit_singleline(&mut self.target);
            });
            ui.add(egui::TextEdit::singleline(&mut self.title).hint_text("Title"));
            ui.add(
                egui::TextEdit::multiline(&mut self.body)
                    .desired_rows(3)
                    .hint_text("Description"),
            );
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        self.pending.is_none()
                            && !self.source.trim().is_empty()
                            && !self.target.trim().is_empty()
                            && !self.title.trim().is_empty(),
                        egui::Button::new("Confirm and create"),
                    )
                    .clicked()
                {
                    self.confirm_create = false;
                    self.request(ApiAction::Create {
                        source: self.source.clone(),
                        target: self.target.clone(),
                        title: self.title.clone(),
                        body: self.body.clone(),
                    });
                }
                if ui.button("Cancel").clicked() {
                    self.confirm_create = false;
                }
            });
        }
        self.poll();
        if self.pending.is_some() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(80));
        }
        if !self.status.is_empty() {
            ui.label(&self.status);
        }
        ui.separator();
        for (i, r) in self.reviews.clone().iter().enumerate() {
            if ui
                .selectable_label(
                    self.selected == Some(i),
                    format!(
                        "#{} · {} · {} → {} · {}",
                        r.id, r.title, r.source, r.target, r.state
                    ),
                )
                .clicked()
            {
                self.selected = Some(i);
                self.request(ApiAction::Detail(r.clone()));
            }
        }
        if self.has_more_reviews
            && ui
                .add_enabled(
                    self.pending.is_none(),
                    egui::Button::new("Load next review page"),
                )
                .clicked()
        {
            self.reviews_page += 1;
            self.request(ApiAction::List {
                page: self.reviews_page,
            });
        }
        if let Some(r) = &self.detail {
            ui.heading(format!("{} — {}", r.id, r.title));
            ui.label(format!("{} → {} · {}", r.source, r.target, r.state));
            ui.label(&r.body);
        }
    }
    fn request(&mut self, action: ApiAction) {
        let config = self.config.clone();
        let token = self.token.clone();
        let (tx, rx) = mpsc::channel();
        self.status = "Request in progress…".into();
        std::thread::spawn(move || {
            let result = execute_action(&config, &token, action);
            let _ = tx.send(result);
        });
        self.pending = Some(rx);
    }
    fn poll(&mut self) {
        if let Some(rx) = &self.pending {
            match rx.try_recv() {
                Ok(result) => {
                    self.pending = None;
                    match result {
                        Ok(r) => {
                            if r.append_reviews || !r.reviews.is_empty() {
                                self.has_more_reviews = r.reviews.len() == 50;
                            }
                            if !r.reviews.is_empty() {
                                if r.append_reviews {
                                    self.reviews.extend(r.reviews);
                                } else {
                                    self.reviews = r.reviews;
                                }
                            }
                            if r.append_repositories || !r.repositories.is_empty() {
                                self.has_more_repositories = r.repositories.len() == 50;
                            }
                            if !r.repositories.is_empty() {
                                if r.append_repositories {
                                    self.repositories.extend(r.repositories);
                                } else {
                                    self.repositories = r.repositories;
                                }
                            }
                            if r.detail.is_some() {
                                self.detail = r.detail;
                            }
                            self.status = r.message;
                        }
                        Err(e) => self.status = e,
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.pending = None;
                    self.status = "Hosting request worker stopped before returning a result".into();
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
    }
    fn ensure_account(&mut self) {
        if self.accounts.is_empty() {
            self.accounts
                .push((self.config.clone(), self.token.clone()));
            self.active_account = 0;
        }
        if self.active_account >= self.accounts.len() {
            self.active_account = 0;
        }
        self.account_repository_paths
            .resize_with(self.accounts.len(), HashMap::new);
    }
    fn save_active(&mut self) {
        self.save_account(self.active_account)
    }
    fn save_account(&mut self, index: usize) {
        if let Some(slot) = self.accounts.get_mut(index) {
            *slot = (self.config.clone(), self.token.clone());
        }
    }
    fn load_active(&mut self) {
        if let Some((c, t)) = self.accounts.get(self.active_account).cloned() {
            self.config = c;
            self.token = t;
        }
    }
    fn clear_remote_data(&mut self) {
        self.pending = None;
        self.reviews.clear();
        self.repositories.clear();
        self.detail = None;
        self.selected = None;
        self.repository_search.clear();
        self.confirm_create = false;
        self.status.clear();
        self.reviews_page = 1;
        self.repositories_page = 1;
        self.has_more_reviews = false;
        self.has_more_repositories = false;
        self.selected_repository_key = None;
        self.selected_local_path.clear();
        self.local_path_valid = false;
        self.open_request = None;
        self.clone_request = None;
    }
    fn clear_reviews(&mut self) {
        self.pending = None;
        self.confirm_create = false;
        self.status.clear();
        self.reviews.clear();
        self.detail = None;
        self.selected = None;
        self.reviews_page = 1;
        self.has_more_reviews = false;
    }
    pub(crate) fn take_open_request(&mut self) -> Option<PathBuf> {
        self.open_request.take()
    }
    pub(crate) fn take_clone_request(&mut self) -> Option<(String, String)> {
        self.clone_request.take()
    }
    fn store_selected_mapping(&mut self, key: String, path: String) {
        if let Some(mappings) = self.account_repository_paths.get_mut(self.active_account) {
            mappings.insert(key, path);
        }
    }
    fn selected_repository_ui(&mut self, ui: &mut egui::Ui) {
        let Some(key) = self.selected_repository_key.clone() else {
            return;
        };
        let Some(repo) = self
            .repositories
            .iter()
            .find(|r| remote_repository_key(self.config.provider, r) == key)
            .cloned()
        else {
            return;
        };
        ui.group(|ui| {
            ui.label(format!("Local copy for {}", repo.full_name));
            ui.horizontal(|ui| {
                ui.label(if self.selected_local_path.is_empty() {
                    "No local copy mapped".into()
                } else {
                    self.selected_local_path.clone()
                });
                if ui
                    .add_enabled(
                        self.pending.is_none(),
                        egui::Button::new("Choose existing local copy…"),
                    )
                    .clicked()
                    && let Some(folder) = rfd::FileDialog::new().pick_folder()
                {
                    match validate_local_repository(&folder) {
                        Ok(canonical) => {
                            self.selected_local_path = canonical.display().to_string();
                            self.local_path_valid = true;
                            self.store_selected_mapping(
                                key.clone(),
                                self.selected_local_path.clone(),
                            );
                            self.status =
                                "Local repository mapped to this remote account repository.".into();
                        }
                        Err(error) => {
                            self.local_path_valid = false;
                            self.status = error;
                        }
                    }
                }
                if ui
                    .add_enabled(self.local_path_valid, egui::Button::new("Open local copy"))
                    .clicked()
                {
                    match validate_local_repository(Path::new(&self.selected_local_path)) {
                        Ok(path) => self.open_request = Some(path),
                        Err(error) => {
                            self.local_path_valid = false;
                            self.status = error;
                        }
                    }
                }
            });
            ui.horizontal(|ui| {
                ui.checkbox(&mut self.clone_ssh, "Use SSH for clone");
                let url = if self.clone_ssh {
                    &repo.ssh
                } else {
                    &repo.https
                };
                if ui
                    .add_enabled(
                        self.pending.is_none()
                            && safe_clone_url(url).is_some()
                            && safe_clone_name(&repo.name),
                        egui::Button::new("Clone repository…"),
                    )
                    .clicked()
                    && let Some(safe_url) = safe_clone_url(url)
                    && safe_clone_name(&repo.name)
                {
                    self.clone_request = Some((safe_url, repo.name.clone()));
                }
            });
        });
    }
}
fn config_from_strings(values: &[String]) -> ApiConfig {
    ApiConfig {
        provider: values
            .first()
            .and_then(|v| v.parse::<usize>().ok())
            .filter(|v| *v < HostingProvider::ALL.len())
            .unwrap_or(0),
        base: values.get(1).cloned().unwrap_or_default(),
        owner: values.get(2).cloned().unwrap_or_default(),
        repo: values.get(3).cloned().unwrap_or_default(),
        username: values.get(4).cloned().unwrap_or_default(),
    }
}
fn remote_repository_key(provider: usize, repo: &HostRepository) -> String {
    if provider == HostingProvider::Beanstalk as usize && !repo.id.is_empty() {
        format!("beanstalk:id:{}", repo.id)
    } else {
        format!("provider:{provider}:{}", repo.full_name)
    }
}
fn validate_local_repository(path: &Path) -> Result<PathBuf, String> {
    let canonical = path
        .canonicalize()
        .map_err(|_| "Choose an existing local Git repository folder".to_owned())?;
    if !canonical.is_dir() {
        return Err("The selected local copy is not a directory".into());
    }
    let output = Command::new("git")
        .arg("-C")
        .arg(&canonical)
        .args(["rev-parse", "--absolute-git-dir"])
        .output()
        .map_err(|_| "Could not run system Git to validate the selected folder".to_owned())?;
    if !output.status.success() {
        return Err("The selected folder is not a Git repository".into());
    }
    Ok(canonical)
}
fn safe_clone_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 255
        && name != "."
        && name != ".."
        && !name.starts_with('-')
        && !name
            .chars()
            .any(|c| c.is_control() || c == '/' || c == '\\')
}
fn safe_clone_url(url: &str) -> Option<String> {
    if url.is_empty()
        || url.len() > 2048
        || url.starts_with('-')
        || url.chars().any(char::is_control)
        || url.contains('?')
        || url.contains('#')
    {
        return None;
    }
    if let Some(rest) = url.strip_prefix("https://") {
        let authority = rest.split('/').next()?;
        let (_, path) = rest.split_once('/')?;
        if authority.is_empty()
            || authority.contains('@')
            || authority.starts_with('-')
            || path.is_empty()
            || path.starts_with('-')
        {
            return None;
        }
        return Some(url.to_owned());
    }
    if let Some(rest) = url.strip_prefix("ssh://") {
        let authority = rest.split('/').next()?;
        let (_, path) = rest.split_once('/')?;
        if authority.is_empty()
            || authority.starts_with('-')
            || path.is_empty()
            || path.starts_with('-')
        {
            return None;
        }
        if let Some((user, _host)) = authority.rsplit_once('@') {
            if user.is_empty() || user.contains(':') || _host.is_empty() {
                return None;
            }
        } else if authority.contains('@') {
            return None;
        }
        return Some(url.to_owned());
    }
    // SCP style: [user@]host:path. Reject password-bearing userinfo and option-like paths.
    let (authority, path) = url.split_once(':')?;
    if authority.is_empty()
        || path.is_empty()
        || authority.starts_with('-')
        || path.starts_with('-')
        || authority.contains('/')
    {
        return None;
    }
    if let Some((user, host)) = authority.rsplit_once('@') {
        if user.is_empty() || user.contains(':') || host.is_empty() || host.starts_with('-') {
            return None;
        }
    } else if authority.contains('@') {
        return None;
    }
    Some(url.to_owned())
}
#[derive(Clone)]
enum ApiAction {
    List {
        page: usize,
    },
    Repositories {
        page: usize,
    },
    Detail(ReviewItem),
    Create {
        source: String,
        target: String,
        title: String,
        body: String,
    },
}

fn execute_action(c: &ApiConfig, token: &str, action: ApiAction) -> Result<ApiResult, String> {
    let p = *HostingProvider::ALL
        .get(c.provider)
        .ok_or("Invalid provider")?;
    if !valid_base(&c.base, p) {
        return Err("API base must be a valid HTTPS URL for this provider".into());
    }
    if token.is_empty() {
        return Err("Enter a session token before requesting the API".into());
    }
    if token.len() > 4096
        || token.chars().any(char::is_control)
        || c.username.chars().any(char::is_control)
    {
        return Err("Token and username must not contain control characters and must be within the supported length".into());
    }
    if p == HostingProvider::Beanstalk && c.username.trim().is_empty() {
        return Err("Beanstalk requires the account username with its access token".into());
    }
    let prefix = p.api_prefix(&c.base);
    let is_detail = matches!(&action, ApiAction::Detail(_));
    let append_reviews = matches!(&action,ApiAction::List{page} if *page>1);
    if let ApiAction::Repositories { page } = &action {
        let url = repository_endpoint(p, &prefix, *page);
        let response = curl_request(p, c, token, "GET", &url, None)?;
        let json: serde_json::Value = serde_json::from_slice(&response)
            .map_err(|_| "Hosting API returned invalid repository JSON".to_owned())?;
        return Ok(ApiResult {
            reviews: Vec::new(),
            detail: None,
            repositories: parse_repositories(p, &json),
            message: "Repositories loaded. Select one to use its account and repository details."
                .into(),
            append_reviews: false,
            append_repositories: *page > 1,
        });
    }
    let (owner, repo) = if p == HostingProvider::Beanstalk {
        (String::new(), path_segment(&c.repo)?)
    } else if p == HostingProvider::GitLab {
        (gitlab_project_path(&c.owner, &c.repo)?, String::new())
    } else {
        (path_segment(&c.owner)?, path_segment(&c.repo)?)
    };
    let (method, url, body) = match action {
        ApiAction::List { page } => (
            "GET",
            endpoint(p, &prefix, &owner, &repo, page),
            None::<String>,
        ),
        ApiAction::Repositories { .. } => {
            unreachable!("handled before pull-request endpoint selection")
        }
        ApiAction::Detail(r) => (
            "GET",
            detail_endpoint(p, &prefix, &owner, &repo, &r.id)?,
            None,
        ),
        ApiAction::Create {
            source,
            target,
            title,
            body,
        } => {
            let source = source.trim();
            let target = target.trim();
            let title = title.trim();
            if source.is_empty() || target.is_empty() || title.is_empty() {
                return Err("Source, target, and title are required".into());
            }
            let payload = create_payload(p, source, target, title, &body, &owner);
            let url = endpoint(p, &prefix, &owner, &repo, 1)
                .split('?')
                .next()
                .unwrap_or_default()
                .to_owned();
            let text = serde_json::to_string(&payload)
                .map_err(|_| "Could not encode review request".to_owned())?;
            let response = curl_request(p, c, token, "POST", &url, Some(&text))?;
            let json: serde_json::Value = serde_json::from_slice(&response)
                .map_err(|_| "Hosting API returned invalid JSON".to_owned())?;
            let review = parse_reviews(p, &json).into_iter().next();
            return Ok(ApiResult {
                reviews: Vec::new(),
                detail: review,
                message: "Review created successfully.".into(),
                repositories: Vec::new(),
                append_reviews: false,
                append_repositories: false,
            });
        }
    };
    let response = curl_request(p, c, token, method, &url, body.as_deref())?;
    let json: serde_json::Value = serde_json::from_slice(&response)
        .map_err(|_| "Hosting API returned invalid or oversized JSON".to_owned())?;
    let reviews = parse_reviews(p, &json);
    let detail = if is_detail {
        reviews.first().cloned()
    } else {
        None
    };
    Ok(ApiResult {
        reviews: if detail.is_some() {
            Vec::new()
        } else {
            reviews
        },
        detail,
        message: "Hosting data loaded.".into(),
        repositories: Vec::new(),
        append_reviews,
        append_repositories: false,
    })
}

fn endpoint(p: HostingProvider, prefix: &str, owner: &str, repo: &str, page: usize) -> String {
    match p {
        HostingProvider::GitHub => {
            format!("{prefix}/repos/{owner}/{repo}/pulls?state=all&per_page=50&page={page}")
        }
        HostingProvider::GitLab => {
            format!("{prefix}/projects/{owner}/merge_requests?state=all&per_page=50&page={page}")
        }
        HostingProvider::Gitea => {
            format!("{prefix}/repos/{owner}/{repo}/pulls?state=all&limit=50&page={page}")
        }
        HostingProvider::Bitbucket => format!(
            "{prefix}/repositories/{owner}/{repo}/pullrequests?state=OPEN&state=MERGED&state=DECLINED&state=SUPERSEDED&pagelen=50&page={page}"
        ),
        HostingProvider::Beanstalk => {
            format!("{prefix}/api/{repo}/code_reviews.json?per_page=50&page={page}")
        }
    }
}
fn gitlab_project_path(owner: &str, repo: &str) -> Result<String, String> {
    if repo.is_empty() {
        return Err("Choose a repository before loading merge requests".into());
    }
    let groups = owner
        .split('/')
        .map(path_segment)
        .collect::<Result<Vec<_>, _>>()?;
    let repo = path_segment(repo)?;
    if groups.is_empty() {
        return Err("Enter the GitLab group or namespace".into());
    }
    Ok(format!("{}%2F{repo}", groups.join("%2F")))
}
fn create_payload(
    p: HostingProvider,
    source: &str,
    target: &str,
    title: &str,
    body: &str,
    owner: &str,
) -> serde_json::Value {
    match p {
        HostingProvider::GitHub => {
            serde_json::json!({"head":format!("{owner}:{source}"),"base":target,"title":title,"body":body,"maintainer_can_modify":true})
        }
        HostingProvider::Gitea => {
            serde_json::json!({"head":format!("{owner}:{source}"),"base":target,"title":title,"body":body})
        }
        HostingProvider::GitLab => {
            serde_json::json!({"source_branch":source,"target_branch":target,"title":title,"description":body})
        }
        HostingProvider::Bitbucket => {
            serde_json::json!({"title":title,"description":body,"source":{"branch":{"name":source}},"destination":{"branch":{"name":target}}})
        }
        HostingProvider::Beanstalk => {
            let description = if body.is_empty() {
                title.to_owned()
            } else {
                format!("{title}\n\n{body}")
            };
            serde_json::json!({"code_review":{"source_branch":target,"target_branch":source,"description":description}})
        }
    }
}
fn repository_endpoint(p: HostingProvider, prefix: &str, page: usize) -> String {
    match p {
        HostingProvider::GitHub => format!("{prefix}/user/repos?per_page=50&page={page}"),
        HostingProvider::GitLab => {
            format!("{prefix}/projects?membership=true&per_page=50&page={page}")
        }
        HostingProvider::Gitea => format!("{prefix}/user/repos?limit=50&page={page}"),
        HostingProvider::Bitbucket => format!(
            "{prefix}/user/permissions/repositories?sort=repository.name&pagelen=50&page={page}"
        ),
        HostingProvider::Beanstalk => {
            format!("{prefix}/api/repositories.json?per_page=50&page={page}")
        }
    }
}
fn parse_repositories(p: HostingProvider, v: &serde_json::Value) -> Vec<HostRepository> {
    let rows = v
        .as_array()
        .or_else(|| v.get("values").and_then(|x| x.as_array()))
        .or_else(|| v.get("repositories").and_then(|x| x.as_array()));
    rows.into_iter()
        .flatten()
        .filter_map(|row| {
            let r = if p == HostingProvider::Bitbucket || p == HostingProvider::Beanstalk {
                row.get("repository").unwrap_or(row)
            } else {
                row
            };
            let val = |path: &str| {
                r.pointer(path)
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_owned()
            };
            let name = val(if p == HostingProvider::GitLab {
                "/path"
            } else {
                "/name"
            });
            let full_name = match p {
                HostingProvider::GitHub | HostingProvider::Gitea | HostingProvider::Bitbucket => {
                    val("/full_name")
                }
                HostingProvider::GitLab => val("/path_with_namespace"),
                HostingProvider::Beanstalk => name.clone(),
            };
            let https = match p {
                HostingProvider::GitHub => val("/clone_url"),
                HostingProvider::GitLab => val("/http_url_to_repo"),
                HostingProvider::Gitea => val("/clone_url"),
                HostingProvider::Bitbucket => clone_link(r, "https"),
                HostingProvider::Beanstalk => val("/repository_url_https"),
            };
            let ssh = match p {
                HostingProvider::GitHub => val("/ssh_url"),
                HostingProvider::GitLab => val("/ssh_url_to_repo"),
                HostingProvider::Gitea => val("/ssh_url"),
                HostingProvider::Bitbucket => clone_link(r, "ssh"),
                HostingProvider::Beanstalk => val("/repository_url"),
            };
            let https = safe_clone_url(&https).unwrap_or_default();
            let ssh = safe_clone_url(&ssh).unwrap_or_default();
            if p == HostingProvider::Beanstalk && val("/vcs") != "git" {
                return None;
            }
            let id = r
                .get("id")
                .map(|x| x.to_string().trim_matches('"').to_owned())
                .unwrap_or_default();
            Some(HostRepository {
                id,
                name,
                full_name,
                https,
                ssh,
            })
        })
        .collect()
}
fn clone_link(r: &serde_json::Value, scheme: &str) -> String {
    r.pointer("/links/clone")
        .and_then(|x| x.as_array())
        .and_then(|a| {
            a.iter()
                .find(|x| x.get("name").and_then(|v| v.as_str()) == Some(scheme))
        })
        .and_then(|x| x.get("href"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_owned()
}
fn detail_endpoint(
    p: HostingProvider,
    prefix: &str,
    owner: &str,
    repo: &str,
    id: &str,
) -> Result<String, String> {
    let id = path_segment(id)?;
    Ok(match p {
        HostingProvider::GitHub => format!("{prefix}/repos/{owner}/{repo}/pulls/{id}"),
        HostingProvider::GitLab => format!("{prefix}/projects/{owner}/merge_requests/{id}"),
        HostingProvider::Gitea => format!("{prefix}/repos/{owner}/{repo}/pulls/{id}"),
        HostingProvider::Bitbucket => {
            format!("{prefix}/repositories/{owner}/{repo}/pullrequests/{id}")
        }
        HostingProvider::Beanstalk => format!("{prefix}/api/{repo}/code_reviews/{id}.json"),
    })
}

fn parse_reviews(p: HostingProvider, v: &serde_json::Value) -> Vec<ReviewItem> {
    let arr = v
        .as_array()
        .or_else(|| v.get("values").and_then(|v| v.as_array()))
        .or_else(|| v.get("code_reviews").and_then(|v| v.as_array()));
    let singleton;
    let items = if let Some(arr) = arr {
        arr.as_slice()
    } else {
        singleton = vec![v.clone()];
        singleton.as_slice()
    };
    items
        .iter()
        .map(|x| {
            let get = |keys: &[&str]| {
                keys.iter()
                    .find_map(|k| x.get(*k).and_then(serde_json::Value::as_str))
            };
            let (source, target) = match p {
                HostingProvider::GitHub | HostingProvider::Gitea => (
                    x.pointer("/head/ref")
                        .and_then(|v| v.as_str())
                        .unwrap_or(""),
                    x.pointer("/base/ref")
                        .and_then(|v| v.as_str())
                        .unwrap_or(""),
                ),
                HostingProvider::GitLab => (
                    get(&["source_branch"]).unwrap_or(""),
                    get(&["target_branch"]).unwrap_or(""),
                ),
                HostingProvider::Bitbucket => (
                    x.pointer("/source/branch/name")
                        .and_then(|v| v.as_str())
                        .unwrap_or(""),
                    x.pointer("/destination/branch/name")
                        .and_then(|v| v.as_str())
                        .unwrap_or(""),
                ),
                HostingProvider::Beanstalk => (
                    get(&["target_branch"]).unwrap_or(""),
                    get(&["source_branch"]).unwrap_or(""),
                ),
            };
            let description = get(&["body", "description"]).unwrap_or("");
            let title = if p == HostingProvider::Beanstalk {
                description
                    .lines()
                    .next()
                    .filter(|s| !s.trim().is_empty())
                    .unwrap_or("(untitled)")
            } else {
                get(&["title", "name"]).unwrap_or("(untitled)")
            };
            ReviewItem {
                id: x
                    .get("number")
                    .or_else(|| x.get("iid"))
                    .or_else(|| x.get("id"))
                    .map(|v| v.to_string().trim_matches('"').to_owned())
                    .unwrap_or_default(),
                title: title.to_owned(),
                source: source.to_owned(),
                target: target.to_owned(),
                state: get(&["state"]).unwrap_or("").to_owned(),
                body: description.to_owned(),
            }
        })
        .collect()
}

fn valid_base(base: &str, p: HostingProvider) -> bool {
    let Some(rest) = base.strip_prefix("https://") else {
        return false;
    };
    !(rest.is_empty()
        || rest.contains('@')
        || rest.contains('?')
        || rest.contains('#')
        || rest.chars().any(char::is_whitespace)
        || p == HostingProvider::Bitbucket
            && base.trim_end_matches('/') != "https://api.bitbucket.org/2.0")
}
fn path_segment(s: &str) -> Result<String, String> {
    if s.is_empty()
        || s.len() > 256
        || s == "."
        || s == ".."
        || s.chars()
            .any(|c| c.is_control() || c == '/' || c == '\\' || c == '?' || c == '#')
    {
        return Err("Owner, repository, and review ID must be single safe path segments".into());
    }
    Ok(percent_encode(s))
}
fn percent_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            out.push(b as char)
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}
fn json_string(s: &str) -> String {
    format!(
        "\"{}\"",
        s.replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n")
            .replace('\r', "\\r")
    )
}
fn curl_request(
    p: HostingProvider,
    c: &ApiConfig,
    token: &str,
    method: &str,
    url: &str,
    body: Option<&str>,
) -> Result<Vec<u8>, String> {
    let test_local = cfg!(test)
        && url.starts_with("http://127.0.0.1:")
        && c.base.starts_with("http://127.0.0.1:");
    if (!url.starts_with("https://") || !valid_base(&c.base, p)) && !test_local {
        return Err("Refusing non-HTTPS API URL".into());
    }
    let mut cfg = format!(
        "url = {}\nrequest = {}\nsilent\nshow-error\nmax-time = 30\nmax-filesize = {}\n",
        json_string(url),
        json_string(method),
        MAX_RESPONSE
    );
    cfg.push_str(&format!(
        "header = {}\nheader = {}\n",
        json_string("Accept: application/json"),
        json_string("User-Agent: grit-native-hosting/1")
    ));
    if let Some(body) = body {
        cfg.push_str(&format!(
            "header = {}\ndata = {}\n",
            json_string("Content-Type: application/json"),
            json_string(body)
        ));
    }
    cfg.push_str(&auth_config(p, c, token));
    let mut child = Command::new("curl")
        .args([
            "--disable",
            "--config",
            "-",
            "--silent",
            "--show-error",
            "--max-time",
            "30",
            "--max-filesize",
            "2097152",
            "--write-out",
            "\n%{http_code}",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "Could not start HTTPS transport (curl)".to_owned())?;
    child
        .stdin
        .take()
        .ok_or("HTTPS transport input unavailable")?
        .write_all(cfg.as_bytes())
        .map_err(|_| "Could not provide request to HTTPS transport")?;
    let mut output = Vec::new();
    let mut stdout = child
        .stdout
        .take()
        .ok_or("HTTPS transport output unavailable")?;
    stdout
        .by_ref()
        .take((MAX_RESPONSE + 32) as u64)
        .read_to_end(&mut output)
        .map_err(|_| "Could not read hosting response")?;
    if output.len() > MAX_RESPONSE + 31 {
        drop(stdout);
        let _ = child.kill();
        let _ = child.wait();
        return Err("Hosting API response exceeded 2 MiB".into());
    }
    let status = child
        .wait()
        .map_err(|_| "HTTPS request failed".to_owned())?;
    if !status.success() {
        return Err(
            "HTTPS request failed (transport, timeout, TLS, or response size limit)".into(),
        );
    }
    let split = output
        .iter()
        .rposition(|b| *b == b'\n')
        .ok_or("Hosting API response was malformed")?;
    if output.len() - split > 8 {
        return Err("Hosting API response was malformed".into());
    }
    let code = std::str::from_utf8(&output[split + 1..])
        .ok()
        .and_then(|s| s.trim().parse::<u16>().ok())
        .ok_or("Hosting API response was malformed")?;
    if !(200..300).contains(&code) {
        return Err(format!(
            "Hosting API returned HTTP {code}; details hidden to protect credentials and repository data"
        ));
    }
    if split > MAX_RESPONSE {
        return Err("Hosting API response exceeded 2 MiB".into());
    }
    Ok(output[..split].to_vec())
}
fn auth_config(p: HostingProvider, c: &ApiConfig, token: &str) -> String {
    match p {
        HostingProvider::GitHub => format!(
            "header = {}\n",
            json_string(&format!("Authorization: Bearer {token}"))
        ),
        HostingProvider::GitLab => format!(
            "header = {}\n",
            json_string(&format!("PRIVATE-TOKEN: {token}"))
        ),
        HostingProvider::Gitea => format!(
            "header = {}\n",
            json_string(&format!("Authorization: token {token}"))
        ),
        HostingProvider::Bitbucket | HostingProvider::Beanstalk => {
            format!(
                "user = {}\n",
                json_string(&format!("{}:{token}", c.username))
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn encoded_paths_and_urls_cannot_inject_routes() {
        assert_eq!(path_segment("org name").unwrap(), "org%20name");
        assert!(path_segment("../other").is_err());
        assert!(!valid_base("http://example.com", HostingProvider::GitHub));
        assert!(!valid_base(
            "https://u:p@example.com",
            HostingProvider::GitHub
        ));
        assert_eq!(
            HostingProvider::GitHub.api_prefix("https://github.com"),
            "https://api.github.com"
        );
        assert_eq!(
            HostingProvider::GitHub.api_prefix("https://github.enterprise"),
            "https://github.enterprise/api/v3"
        );
        assert_eq!(
            gitlab_project_path("group/subgroup", "repo").unwrap(),
            "group%2Fsubgroup%2Frepo"
        );
        assert!(gitlab_project_path("group/subgroup", "../repo").is_err());
    }
    #[test]
    fn provider_endpoints_match_api_families() {
        let pre = "https://host/api";
        assert!(endpoint(HostingProvider::GitHub, pre, "a", "b", 1).contains("/repos/a/b/pulls"));
        assert!(
            endpoint(
                HostingProvider::GitLab,
                pre,
                &gitlab_project_path("group/sub", "project").unwrap(),
                "",
                1
            )
            .contains("/projects/group%2Fsub%2Fproject/merge_requests")
        );
        assert!(endpoint(HostingProvider::Gitea, pre, "a", "b", 2).contains("page=2"));
        assert!(
            endpoint(
                HostingProvider::Bitbucket,
                "https://api.bitbucket.org/2.0",
                "a",
                "b",
                1
            )
            .contains("/pullrequests?state=OPEN&state=MERGED&state=DECLINED&state=SUPERSEDED")
        );
        assert!(
            endpoint(HostingProvider::Beanstalk, pre, "", "b", 1)
                .ends_with("/api/b/code_reviews.json?per_page=50&page=1")
        );
    }
    #[test]
    fn repository_endpoints_cover_account_browsing() {
        assert!(
            repository_endpoint(HostingProvider::GitHub, "https://api.github.com", 1)
                .contains("/user/repos")
        );
        assert!(
            repository_endpoint(HostingProvider::GitLab, "https://gitlab.com/api/v4", 1)
                .contains("/projects?")
        );
        assert!(
            repository_endpoint(HostingProvider::Gitea, "https://gitea.example/api/v1", 1)
                .contains("/user/repos")
        );
        assert!(
            repository_endpoint(
                HostingProvider::Bitbucket,
                "https://api.bitbucket.org/2.0",
                1
            )
            .contains("/user/permissions/repositories")
        );
        assert!(
            repository_endpoint(
                HostingProvider::Beanstalk,
                "https://acme.beanstalkapp.com",
                1
            )
            .ends_with("/api/repositories.json?per_page=50&page=1")
        );
    }
    #[test]
    fn request_bodies_match_all_provider_contracts() {
        assert_eq!(
            create_payload(
                HostingProvider::GitHub,
                "feature",
                "main",
                "T",
                "B",
                "alice"
            ),
            serde_json::json!({"head":"alice:feature","base":"main","title":"T","body":"B","maintainer_can_modify":true})
        );
        assert_eq!(
            create_payload(
                HostingProvider::GitLab,
                "feature",
                "main",
                "T",
                "B",
                "alice"
            ),
            serde_json::json!({"source_branch":"feature","target_branch":"main","title":"T","description":"B"})
        );
        assert_eq!(
            create_payload(HostingProvider::Gitea, "feature", "main", "T", "B", "alice"),
            serde_json::json!({"head":"alice:feature","base":"main","title":"T","body":"B"})
        );
        assert_eq!(
            create_payload(
                HostingProvider::Bitbucket,
                "feature",
                "main",
                "T",
                "B",
                "alice"
            ),
            serde_json::json!({"title":"T","description":"B","source":{"branch":{"name":"feature"}},"destination":{"branch":{"name":"main"}}})
        );
        assert_eq!(
            create_payload(
                HostingProvider::Beanstalk,
                "feature",
                "main",
                "T",
                "B",
                "alice"
            ),
            serde_json::json!({"code_review":{"source_branch":"main","target_branch":"feature","description":"T\n\nB"}})
        );
        assert_eq!(
            create_payload(
                HostingProvider::Beanstalk,
                "feature",
                "main",
                "T",
                "",
                "alice"
            )["code_review"]["description"],
            "T"
        );
    }
    #[test]
    fn auth_policy_keeps_provider_tokens_out_of_urls() {
        let c = ApiConfig {
            username: "user@example.test".into(),
            ..ApiConfig::default()
        };
        assert!(
            auth_config(HostingProvider::GitHub, &c, "secret")
                .contains("Authorization: Bearer secret")
        );
        assert!(
            auth_config(HostingProvider::GitLab, &c, "secret").contains("PRIVATE-TOKEN: secret")
        );
        for p in [HostingProvider::Bitbucket, HostingProvider::Beanstalk] {
            let a = auth_config(p, &c, "secret");
            assert!(a.contains("user@example.test:secret"));
            assert!(!a.contains("Authorization:"));
        }
        assert!(
            auth_config(HostingProvider::Gitea, &c, "secret")
                .contains("Authorization: token secret")
        );
    }
    #[test]
    fn parser_adapts_provider_shapes() {
        let v: serde_json::Value = serde_json::json!([{"number":4,"title":"Review","head":{"ref":"topic"},"base":{"ref":"main"},"body":"details","state":"open"}]);
        let r = parse_reviews(HostingProvider::GitHub, &v);
        assert_eq!(r[0].source, "topic");
        assert_eq!(r[0].target, "main");
        assert_eq!(r[0].body, "details");
    }
    #[test]
    fn repository_parser_handles_provider_shapes_and_filters_non_git_beanstalk() {
        let bitbucket = serde_json::json!({"values":[{"repository":{"id":7,"name":"repo","full_name":"team/repo","links":{"clone":[{"name":"https","href":"https://bitbucket.org/team/repo"},{"name":"ssh","href":"git@bitbucket.org:team/repo.git"}]}}}]});
        let b = parse_repositories(HostingProvider::Bitbucket, &bitbucket);
        assert_eq!(b[0].https, "https://bitbucket.org/team/repo");
        assert_eq!(b[0].id, "7");
        let bean = serde_json::json!([{"repository":{"id":1,"vcs":"git","name":"repo","repository_url_https":"https://acme.beanstalkapp.com/repo","repository_url":"git@acme.beanstalkapp.com:/repo.git"}},{"repository":{"id":2,"vcs":"svn","name":"svn"}}]);
        let b = parse_repositories(HostingProvider::Beanstalk, &bean);
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].id, "1");
        assert_eq!(b[0].ssh, "git@acme.beanstalkapp.com:/repo.git");
    }
    #[test]
    fn token_is_not_persisted_or_in_error_text() {
        let t = HostingApiTools {
            token: "test-secret".into(),
            ..HostingApiTools::default()
        };
        assert!(
            t.persistence_strings()
                .iter()
                .all(|s| !s.contains("test-secret"))
        );
        assert!(
            curl_request(
                HostingProvider::GitHub,
                &ApiConfig::default(),
                "secret",
                "GET",
                "http://invalid",
                None
            )
            .unwrap_err()
            .contains("HTTPS")
        );
    }
    #[test]
    fn account_metadata_roundtrips_without_tokens_and_rejects_credential_urls() {
        let mut tools = HostingApiTools::default();
        tools.config = ApiConfig {
            provider: 0,
            base: "https://api.github.com".into(),
            owner: "alice".into(),
            repo: "repo".into(),
            username: "alice".into(),
        };
        tools.token = "secret-token".into();
        tools.accounts = vec![(tools.config.clone(), tools.token.clone())];
        tools.accounts.push((
            ApiConfig {
                provider: 4,
                base: "https://user:password@acme.beanstalkapp.com".into(),
                owner: String::new(),
                repo: "41".into(),
                username: "acme".into(),
            },
            "another-secret".into(),
        ));
        let stored = tools.persistence_strings();
        let serialized = stored.join(" ");
        assert!(!serialized.contains("secret-token"));
        assert!(!serialized.contains("another-secret"));
        assert!(!serialized.contains("password"));
        let mut loaded = HostingApiTools::default();
        loaded.load_persistence_strings(&stored);
        assert_eq!(loaded.accounts.len(), 2);
        assert_eq!(loaded.accounts[0].0.owner, "alice");
        assert!(loaded.accounts[1].0.base.is_empty());
        assert!(loaded.accounts.iter().all(|(_, token)| token.is_empty()));
    }
    #[test]
    fn local_path_mappings_roundtrip_per_account_and_exact_remote_repository() {
        let mut tools = HostingApiTools::default();
        let first = HostRepository {
            id: "".into(),
            name: "one".into(),
            full_name: "alice/one".into(),
            ..HostRepository::default()
        };
        let second = HostRepository {
            id: "".into(),
            name: "two".into(),
            full_name: "alice/two".into(),
            ..HostRepository::default()
        };
        let account_zero = remote_repository_key(0, &first);
        let account_one = remote_repository_key(0, &second);
        tools.config = ApiConfig {
            provider: 0,
            base: "https://api.github.com".into(),
            username: "alice".into(),
            ..ApiConfig::default()
        };
        tools.accounts = vec![
            (tools.config.clone(), "secret-a".into()),
            (
                ApiConfig {
                    provider: 0,
                    base: "https://ghe.example".into(),
                    username: "alice".into(),
                    ..ApiConfig::default()
                },
                "secret-b".into(),
            ),
        ];
        tools.account_repository_paths = vec![
            HashMap::from([
                (account_zero.clone(), "/tmp/account-zero-one".into()),
                (account_one.clone(), "/tmp/account-zero-two".into()),
            ]),
            HashMap::from([(account_zero.clone(), "/tmp/account-one-one".into())]),
        ];
        let stored = tools.persistence_strings();
        let text = stored.join(" ");
        assert!(!text.contains("secret-a"));
        assert!(!text.contains("secret-b"));
        let mut loaded = HostingApiTools::default();
        loaded.load_persistence_strings(&stored);
        assert_eq!(
            loaded.account_repository_paths[0]
                .get(&account_zero)
                .unwrap(),
            "/tmp/account-zero-one"
        );
        assert_eq!(
            loaded.account_repository_paths[0]
                .get(&account_one)
                .unwrap(),
            "/tmp/account-zero-two"
        );
        assert_eq!(
            loaded.account_repository_paths[1]
                .get(&account_zero)
                .unwrap(),
            "/tmp/account-one-one"
        );
        loaded.active_account = 1;
        loaded.selected_repository_key = Some(remote_repository_key(0, &second));
        assert!(
            !loaded.account_repository_paths[loaded.active_account]
                .contains_key(loaded.selected_repository_key.as_deref().unwrap())
        );
    }
    #[test]
    fn repository_switch_discards_stale_detail_and_confirmation_but_detail_keeps_list() {
        let item = ReviewItem {
            id: "9".into(),
            title: "Old title".into(),
            ..ReviewItem::default()
        };
        let mut tools = HostingApiTools {
            reviews: vec![item.clone()],
            confirm_create: true,
            ..HostingApiTools::default()
        };
        let (tx, rx) = mpsc::channel();
        tx.send(Ok(ApiResult {
            reviews: Vec::new(),
            detail: Some(item.clone()),
            message: "stale".into(),
            repositories: Vec::new(),
            append_reviews: false,
            append_repositories: false,
        }))
        .unwrap();
        tools.pending = Some(rx);
        tools.clear_reviews();
        tools.poll();
        assert!(tools.pending.is_none());
        assert!(!tools.confirm_create);
        assert!(tools.reviews.is_empty());
        assert!(tools.detail.is_none());

        tools.reviews.push(item.clone());
        let (tx, rx) = mpsc::channel();
        tx.send(Ok(ApiResult {
            reviews: Vec::new(),
            detail: Some(item.clone()),
            message: "detail loaded".into(),
            repositories: Vec::new(),
            append_reviews: false,
            append_repositories: false,
        }))
        .unwrap();
        tools.pending = Some(rx);
        tools.poll();
        assert_eq!(tools.reviews, vec![item.clone()]);
        assert_eq!(tools.detail, Some(item));
    }
    #[test]
    fn local_mapping_requires_existing_git_repo_and_clone_urls_are_credential_safe() {
        use std::{
            fs,
            time::{SystemTime, UNIX_EPOCH},
        };
        let id = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("grit-hosting-mapping-{id}"));
        fs::create_dir_all(&dir).unwrap();
        let repo = dir.join("valid repo");
        let output = Command::new("git")
            .args(["init", "--bare", "-q"])
            .arg(&repo)
            .output()
            .unwrap();
        assert!(output.status.success());
        let canonical = validate_local_repository(&repo).unwrap();
        assert!(canonical.is_absolute());
        assert!(validate_local_repository(&dir).is_err());
        assert_eq!(
            safe_clone_url("https://github.com/alice/repo.git").as_deref(),
            Some("https://github.com/alice/repo.git")
        );
        assert_eq!(
            safe_clone_url("git@github.com:alice/repo.git").as_deref(),
            Some("git@github.com:alice/repo.git")
        );
        assert!(safe_clone_url("https://user:password@github.com/alice/repo.git").is_none());
        assert!(safe_clone_url("https://secret@github.com/alice/repo.git").is_none());
        assert!(safe_clone_url("-upload-pack=bad").is_none());
        assert!(safe_clone_url("https://github.com/-option.git").is_none());
        assert!(!safe_clone_name("-leading-dash"));
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn production_curl_transport_works_against_test_only_local_mock() {
        use std::{
            io::{Read, Write},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut req = vec![0; 8192];
            let n = stream.read(&mut req).unwrap();
            let text = String::from_utf8_lossy(&req[..n]);
            assert!(text.contains("Authorization: Bearer mock-token"));
            let body = "[]";
            write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body).unwrap();
        });
        let base = format!("http://{addr}");
        let c = ApiConfig {
            provider: 0,
            base: base.clone(),
            owner: "owner".into(),
            repo: "repo".into(),
            username: String::new(),
        };
        let result = curl_request(
            HostingProvider::GitHub,
            &c,
            "mock-token",
            "GET",
            &format!("{base}/api/v3/repos/owner/repo/pulls"),
            None,
        )
        .unwrap();
        server.join().unwrap();
        assert_eq!(result, b"[]");
    }
    #[test]
    fn production_curl_auth_headers_match_provider_policy_against_local_mocks() {
        use std::{
            io::{Read, Write},
            net::TcpListener,
        };
        for (provider, expected) in [
            (HostingProvider::GitHub, "Authorization: Bearer token"),
            (HostingProvider::GitLab, "PRIVATE-TOKEN: token"),
            (HostingProvider::Gitea, "Authorization: token token"),
            (
                HostingProvider::Bitbucket,
                "Authorization: Basic dXNlcjp0b2tlbg==",
            ),
            (
                HostingProvider::Beanstalk,
                "Authorization: Basic dXNlcjp0b2tlbg==",
            ),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let base = format!("http://{}", listener.local_addr().unwrap());
            let server = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                let mut req = vec![0; 8192];
                let n = stream.read(&mut req).unwrap();
                let text = String::from_utf8_lossy(&req[..n]);
                assert!(
                    text.contains(expected),
                    "missing expected auth header for {provider:?}: {text}"
                );
                let body = "[]";
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                )
                .unwrap();
            });
            let c = ApiConfig {
                base: base.clone(),
                username: "user".into(),
                ..ApiConfig::default()
            };
            curl_request(provider, &c, "token", "GET", &format!("{base}/api"), None).unwrap();
            server.join().unwrap();
        }
    }
}
