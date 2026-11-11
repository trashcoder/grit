use super::*;
use std::sync::Arc;

#[derive(Default)]
pub(super) struct HistoryOptions {
    pub stars: HashMap<String, HashSet<String>>,
    pub only_starred: bool,
    pub first_parent: bool,
    pub sort_by_date: bool,
    pub compact: bool,
    pub hide_graph: bool,
    pub hide_author: bool,
    pub hide_date: bool,
    pub hide_id: bool,
    pub hide_untracked: bool,
    pub navigation: Vec<String>,
    pub navigation_index: usize,
}

pub(super) struct HistoryViewCache {
    by_id: HashMap<String, usize>,
    topology_key: Option<(bool, bool)>,
    topology: Vec<usize>,
    graph: Option<Arc<HistoryGraph>>,
    filter_key: Option<(String, bool)>,
    rows: Result<Arc<HistoryRows>, String>,
}

pub(super) struct HistoryRows {
    pub indices: Vec<usize>,
    pub positions: HashMap<usize, usize>,
}

pub(super) struct HistoryGraph {
    pub rows: Vec<Option<GraphRow>>,
    pub lane_count: usize,
}

impl Default for HistoryViewCache {
    fn default() -> Self {
        Self {
            by_id: HashMap::new(),
            topology_key: None,
            topology: Vec::new(),
            graph: None,
            filter_key: None,
            rows: Ok(Arc::new(HistoryRows {
                indices: Vec::new(),
                positions: HashMap::new(),
            })),
        }
    }
}

impl HistoryViewCache {
    pub(super) fn invalidate_topology(&mut self) {
        self.topology_key = None;
    }

    pub(super) fn commit_index(&self, id: &str) -> Option<usize> {
        self.by_id.get(id).copied()
    }
}

#[derive(Debug)]
enum Term {
    Text(String),
    Author(String),
    Email(String),
    Message(String),
    Id(String),
    Date(chrono::NaiveDate),
    After(chrono::NaiveDate),
    Before(chrono::NaiveDate),
    Path(String),
}
fn parse_query(query: &str) -> Result<Vec<Term>, String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    for c in query.chars() {
        match c {
            '"' => quoted = !quoted,
            c if c.is_whitespace() && !quoted => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
            c => current.push(c),
        }
    }
    if quoted {
        return Err("Close the quotation mark in the search query".into());
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens.into_iter().map(|token| {
        let Some((field, value)) = token.split_once(':') else { return Ok(Term::Text(token.to_lowercase())); };
        if value.is_empty() { return Err(format!("Search field {field} needs a value")); }
        let text = value.to_lowercase();
        Ok(match field {
            "author" => Term::Author(text), "email" => Term::Email(text), "message" => Term::Message(text), "id" => Term::Id(text),
            "path" | "file" => { file_features::validate_repo_path(value)?; Term::Path(value.to_owned()) },
            "date" | "after" | "before" => {
                let date = chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d").map_err(|_| format!("Use YYYY-MM-DD for {field}"))?;
                match field { "date" => Term::Date(date), "after" => Term::After(date), _ => Term::Before(date) }
            }
            _ => return Err(format!("Unknown search field: {field}. Use author, email, message, id, date, after, before or path.")),
        })
    }).collect()
}
fn matches_terms(c: &Commit, terms: &[Term], repo: Option<&Repository>) -> bool {
    let contains = |s: &str, q: &str| s.to_lowercase().contains(q);
    terms.iter().all(|term| match term {
        Term::Text(q) => [&c.full_message, &c.author, &c.author_email, &c.id]
            .iter()
            .any(|s| contains(s, q)),
        Term::Author(q) => contains(&c.author, q),
        Term::Email(q) => contains(&c.author_email, q),
        Term::Message(q) => contains(&c.full_message, q),
        Term::Id(q) => contains(&c.id, q),
        Term::Date(date) => commit_date(c).is_some_and(|d| d == *date),
        Term::After(date) => commit_date(c).is_some_and(|d| d >= *date),
        Term::Before(date) => commit_date(c).is_some_and(|d| d <= *date),
        Term::Path(path) => {
            repo.is_some_and(|r| commit_files(r, &c.id).iter().any(|f| &f.path == path))
        }
    })
}
fn commit_date(c: &Commit) -> Option<chrono::NaiveDate> {
    let offset = chrono::FixedOffset::east_opt(c.time_offset * 60)?;
    Some(
        chrono::DateTime::from_timestamp(c.timestamp, 0)?
            .with_timezone(&offset)
            .date_naive(),
    )
}
pub(super) fn first_parent_chain(commits: &[Commit], start: &str) -> HashSet<String> {
    let by_id: HashMap<_, _> = commits.iter().map(|c| (c.id.as_str(), c)).collect();
    let mut result = HashSet::new();
    let mut next = Some(start);
    while let Some(id) = next {
        if !result.insert(id.to_owned()) {
            break;
        }
        next = by_id
            .get(id)
            .and_then(|c| c.parents.first())
            .map(String::as_str);
    }
    result
}
pub(super) fn repo_storage_key(path: &Path) -> String {
    path.canonicalize()
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .into_owned()
}
impl GritApp {
    pub(super) fn is_starred(&self, id: &str) -> bool {
        self.path
            .as_ref()
            .and_then(|p| self.history_options.stars.get(&repo_storage_key(p)))
            .is_some_and(|ids| ids.contains(id))
    }
    pub(super) fn toggle_star(&mut self, id: &str) {
        let Some(path) = &self.path else {
            return;
        };
        let stars = self
            .history_options
            .stars
            .entry(repo_storage_key(path))
            .or_default();
        if !stars.remove(id) {
            stars.insert(id.to_owned());
        }
        if self.history_options.only_starred {
            self.history_view_cache.filter_key = None;
        }
    }
    pub(super) fn history_rows(&mut self) -> Result<Arc<HistoryRows>, String> {
        let topology_key = (
            self.history_options.first_parent,
            self.history_options.sort_by_date,
        );
        if self.history_view_cache.topology_key != Some(topology_key) {
            let first_parent = if topology_key.0 {
                let start = self.repo().and_then(|repo| {
                    repo.revparse_single(
                        self.history_scope
                            .as_ref()
                            .map(|(name, _)| name.as_str())
                            .unwrap_or("HEAD"),
                    )
                    .and_then(|object| object.peel_to_commit())
                    .ok()
                    .map(|commit| commit.id().to_string())
                });
                start.map(|id| first_parent_chain(&self.commits, &id))
            } else {
                None
            };
            let cache = &mut self.history_view_cache;
            if cache.by_id.is_empty() {
                cache.by_id = self
                    .commits
                    .iter()
                    .enumerate()
                    .map(|(index, commit)| (commit.id.clone(), index))
                    .collect();
            }
            cache.topology = self
                .commits
                .iter()
                .enumerate()
                .filter_map(|(index, commit)| {
                    (self
                        .history_scope
                        .as_ref()
                        .is_none_or(|(_, ids)| ids.contains(&commit.id))
                        && first_parent
                            .as_ref()
                            .is_none_or(|ids| ids.contains(&commit.id)))
                    .then_some(index)
                })
                .collect();
            if topology_key.1 {
                cache
                    .topology
                    .sort_by_key(|&index| std::cmp::Reverse(self.commits[index].timestamp));
            }
            cache.topology_key = Some(topology_key);
            cache.filter_key = None;
            cache.graph = None;
        }

        let only_starred = self.history_options.only_starred;
        if !self
            .history_view_cache
            .filter_key
            .as_ref()
            .is_some_and(|(query, starred)| query == &self.search && *starred == only_starred)
        {
            let result = parse_query(&self.search).map(|terms| {
                let repo = terms
                    .iter()
                    .any(|term| matches!(term, Term::Path(_)))
                    .then(|| self.repo())
                    .flatten();
                let stars = self
                    .path
                    .as_ref()
                    .and_then(|path| self.history_options.stars.get(&repo_storage_key(path)));
                let indices: Vec<_> = self
                    .history_view_cache
                    .topology
                    .iter()
                    .copied()
                    .filter(|&index| {
                        let commit = &self.commits[index];
                        (!only_starred || stars.is_some_and(|ids| ids.contains(&commit.id)))
                            && matches_terms(commit, &terms, repo.as_ref())
                    })
                    .collect();
                let positions = indices
                    .iter()
                    .enumerate()
                    .map(|(row, &index)| (index, row))
                    .collect();
                Arc::new(HistoryRows { indices, positions })
            });
            self.history_view_cache.rows = result;
            self.history_view_cache.filter_key = Some((self.search.clone(), only_starred));
        }
        self.history_view_cache.rows.clone()
    }

    pub(super) fn history_graph(&mut self) -> Option<Arc<HistoryGraph>> {
        if self.history_options.hide_graph {
            return None;
        }
        let cache = &mut self.history_view_cache;
        let graph = cache.graph.get_or_insert_with(|| {
            let commits = cache.topology.iter().map(|&index| &self.commits[index]);
            let graph_rows = graph_layout(commits, self.history_options.first_parent);
            let mut rows: Vec<_> = (0..self.commits.len()).map(|_| None).collect();
            let mut lane_count = 1;
            for (&index, row) in cache.topology.iter().zip(graph_rows) {
                lane_count = lane_count.max(row.occupied.len());
                rows[index] = Some(row);
            }
            Arc::new(HistoryGraph { rows, lane_count })
        });
        Some(Arc::clone(graph))
    }

    pub(super) fn history_display_options(&mut self, ui: &mut egui::Ui) {
        ui.menu_button("View", |ui| {
            ui.checkbox(
                &mut self.history_options.only_starred,
                "Starred commits only",
            );
            ui.checkbox(
                &mut self.history_options.first_parent,
                "First parent only (selected branch / HEAD)",
            );
            ui.checkbox(&mut self.history_options.sort_by_date, "Sort by date");
            ui.separator();
            ui.checkbox(&mut self.history_options.compact, "Compact rows");
            ui.checkbox(&mut self.history_options.hide_graph, "Hide graph");
            ui.checkbox(&mut self.history_options.hide_author, "Hide author");
            ui.checkbox(&mut self.history_options.hide_date, "Hide date");
            ui.checkbox(&mut self.history_options.hide_id, "Hide SHA");
            ui.separator();
            ui.label("Search: author:, email:, message:, id:, path:");
            ui.label("Dates: date:, after:, before:YYYY-MM-DD");
            ui.label("Combine fields; quote values containing spaces.");
        });
    }
    pub(super) fn navigate_history(&mut self, forward: bool) {
        let opts = &mut self.history_options;
        if opts.navigation.is_empty() {
            return;
        }
        opts.navigation_index = if forward {
            (opts.navigation_index + 1).min(opts.navigation.len() - 1)
        } else {
            opts.navigation_index.saturating_sub(1)
        };
        let id = opts.navigation[opts.navigation_index].clone();
        self.search.clear();
        self.history_options.only_starred = false;
        self.history_options.first_parent = false;
        self.set_history_scope(None);
        self.load_history_commit(id);
    }
}

// Length-prefixed UTF-8 records preserve template bodies and names containing newlines.
pub(super) fn encode_records(records: &[String]) -> String {
    records.iter().map(|s| format!("{}:{s}", s.len())).collect()
}
pub(super) fn decode_records(mut input: &str) -> Result<Vec<String>, String> {
    let mut records = Vec::new();
    while !input.is_empty() {
        let (size, rest) = input.split_once(':').ok_or("Invalid saved records")?;
        let len: usize = size.parse().map_err(|_| "Invalid saved length")?;
        let value = rest.get(..len).ok_or("Truncated saved record")?;
        records.push(value.to_owned());
        input = &rest[len..];
    }
    Ok(records)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn query_quotes_dates_and_all_terms_must_match() {
        let c = Commit {
            author: "Ada Lovelace".into(),
            author_email: "ada@example.org".into(),
            full_message: "Fix parser\n\nHandle quoted values".into(),
            timestamp: 1735689600,
            time_offset: 0,
            ..Default::default()
        };
        let q = parse_query("author:\"Ada Lovelace\" email:EXAMPLE after:2025-01-01 before:2025-01-01 message:parser").unwrap();
        assert!(matches_terms(&c, &q, None));
        assert!(!matches_terms(
            &c,
            &parse_query("author:Ada message:missing").unwrap(),
            None
        ));
        assert!(parse_query("after:tomorrow").is_err());
        assert!(parse_query("author:\"open").is_err());
        assert!(parse_query("addition:todo").is_err());
        assert!(parse_query("path:../outside").is_err());
    }
    #[test]
    fn first_parent_excludes_merged_side_branch() {
        let make = |id: &str, parents: &[&str]| Commit {
            id: id.into(),
            parents: parents.iter().map(|p| (*p).into()).collect(),
            ..Default::default()
        };
        let commits = vec![
            make("merge", &["main", "feature"]),
            make("main", &["base"]),
            make("feature", &["base"]),
            make("base", &[]),
        ];
        assert_eq!(
            first_parent_chain(&commits, "merge"),
            HashSet::from(["merge".into(), "main".into(), "base".into()])
        );
    }
    #[test]
    fn template_records_roundtrip_unicode_and_linebreaks() {
        let records = vec![
            "Grüße 🦀".into(),
            "subject\n\nbody: text\n".into(),
            String::new(),
        ];
        assert_eq!(decode_records(&encode_records(&records)).unwrap(), records);
        assert!(decode_records("20:short").is_err());
        assert!(decode_records("x:abc").is_err());
    }
    #[test]
    fn stars_are_repository_specific() {
        let mut app = GritApp {
            path: Some(PathBuf::from("/tmp/repo-star-one")),
            ..Default::default()
        };
        app.toggle_star("abc");
        assert!(app.is_starred("abc"));
        app.path = Some(PathBuf::from("/tmp/repo-star-two"));
        assert!(!app.is_starred("abc"));
        app.path = Some(PathBuf::from("/tmp/repo-star-one"));
        app.toggle_star("abc");
        assert!(!app.is_starred("abc"));
    }
}
