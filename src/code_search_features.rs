use super::*;
use std::{
    collections::{HashMap, HashSet},
    sync::mpsc::{self, Receiver, Sender, TryRecvError},
    thread,
};

const MAX_COMMITS: usize = 2_000;
const MAX_INDEX_LINES: usize = 120_000;
const MAX_INDEX_BYTES: usize = 32 * 1024 * 1024;
const MAX_SINGLE_LINE_BYTES: usize = 256 * 1024;
const MAX_RESULTS: usize = 3_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LineKind {
    Addition,
    Deletion,
    Context,
}

#[derive(Clone, Debug)]
struct IndexLine {
    commit: String,
    path: usize,
    line: u32,
    kind: LineKind,
    text: String,
    hunk: Option<usize>,
}

#[derive(Clone, Debug)]
struct CommitMetadata {
    message: String,
    author: String,
}

#[derive(Default)]
struct SearchIndex {
    lines: Vec<IndexLine>,
    hunks: Vec<String>,
    paths: Vec<String>,
    path_ids: HashMap<String, usize>,
    commits: HashMap<String, CommitMetadata>,
    bytes: usize,
    commits_indexed: usize,
    truncated: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Field {
    Any,
    Addition,
    Deletion,
    Context,
    Comment,
    String,
    Identifier,
    Path,
    Message,
    Author,
    Hunk,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SearchTerm {
    field: Field,
    value: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Expr {
    Term(SearchTerm),
    Not(Box<Expr>),
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Token {
    Word(String),
    And,
    Or,
    Not,
    Left,
    Right,
}

fn tokenize_query(query: &str) -> Result<Vec<Token>, String> {
    let mut tokens = Vec::new();
    let mut word = String::new();
    let mut quoted = false;
    let mut word_was_quoted = false;
    let mut escaped = false;
    let flush = |word: &mut String, word_was_quoted: &mut bool, tokens: &mut Vec<Token>| {
        if word.is_empty() {
            *word_was_quoted = false;
            return;
        }
        let value = std::mem::take(word);
        let token = if *word_was_quoted {
            Token::Word(value)
        } else {
            match value.to_ascii_uppercase().as_str() {
                "AND" => Token::And,
                "OR" => Token::Or,
                "NOT" => Token::Not,
                _ => Token::Word(value),
            }
        };
        *word_was_quoted = false;
        tokens.push(token);
    };
    for ch in query.chars() {
        if escaped {
            word.push(ch);
            escaped = false;
            continue;
        }
        if quoted && ch == '\\' {
            escaped = true;
            continue;
        }
        match ch {
            '"' => {
                quoted = !quoted;
                word_was_quoted = true;
            }
            '(' if !quoted => {
                flush(&mut word, &mut word_was_quoted, &mut tokens);
                tokens.push(Token::Left);
            }
            ')' if !quoted => {
                flush(&mut word, &mut word_was_quoted, &mut tokens);
                tokens.push(Token::Right);
            }
            c if c.is_whitespace() && !quoted => {
                flush(&mut word, &mut word_was_quoted, &mut tokens)
            }
            _ => word.push(ch),
        }
    }
    if quoted || escaped {
        return Err("Close the quotation mark in the query.".into());
    }
    flush(&mut word, &mut word_was_quoted, &mut tokens);
    if tokens.is_empty() {
        return Err("Enter a search query.".into());
    }
    Ok(tokens)
}

struct Parser {
    tokens: Vec<Token>,
    cursor: usize,
}

impl Parser {
    fn parse(mut self) -> Result<Expr, String> {
        let expr = self.parse_or()?;
        if self.cursor != self.tokens.len() {
            return Err("Unexpected operator or closing parenthesis.".into());
        }
        Ok(expr)
    }

    fn parse_or(&mut self) -> Result<Expr, String> {
        let mut expr = self.parse_and()?;
        while self.tokens.get(self.cursor) == Some(&Token::Or) {
            self.cursor += 1;
            let right = self.parse_and()?;
            expr = Expr::Or(Box::new(expr), Box::new(right));
        }
        Ok(expr)
    }

    fn starts_unary(&self) -> bool {
        matches!(
            self.tokens.get(self.cursor),
            Some(Token::Word(_) | Token::Not | Token::Left)
        )
    }

    fn parse_and(&mut self) -> Result<Expr, String> {
        let mut expr = self.parse_unary()?;
        loop {
            if self.tokens.get(self.cursor) == Some(&Token::And) {
                self.cursor += 1;
                let right = self.parse_unary()?;
                expr = Expr::And(Box::new(expr), Box::new(right));
            } else if self.starts_unary() {
                let right = self.parse_unary()?;
                expr = Expr::And(Box::new(expr), Box::new(right));
            } else {
                return Ok(expr);
            }
        }
    }

    fn parse_unary(&mut self) -> Result<Expr, String> {
        if self.tokens.get(self.cursor) == Some(&Token::Not) {
            self.cursor += 1;
            return Ok(Expr::Not(Box::new(self.parse_unary()?)));
        }
        self.parse_primary()
    }

    fn parse_primary(&mut self) -> Result<Expr, String> {
        match self.tokens.get(self.cursor).cloned() {
            Some(Token::Word(word)) => {
                self.cursor += 1;
                Ok(Expr::Term(parse_term(&word)?))
            }
            Some(Token::Left) => {
                self.cursor += 1;
                let expr = self.parse_or()?;
                if self.tokens.get(self.cursor) != Some(&Token::Right) {
                    return Err("Add a closing parenthesis to the query.".into());
                }
                self.cursor += 1;
                Ok(expr)
            }
            Some(Token::Right) => Err("Unexpected closing parenthesis.".into()),
            Some(Token::And | Token::Or) => {
                Err("An operator needs a search term on both sides.".into())
            }
            Some(Token::Not) => Err("NOT needs a search term.".into()),
            None => Err("The query ends before an expected search term.".into()),
        }
    }
}

fn parse_query(query: &str) -> Result<Expr, String> {
    Parser {
        tokens: tokenize_query(query)?,
        cursor: 0,
    }
    .parse()
}

fn parse_term(word: &str) -> Result<SearchTerm, String> {
    let (field, value) = word.split_once(':').unwrap_or(("", word));
    if value.is_empty() {
        return Err(format!("Search field {field} needs a value."));
    }
    let field = match field {
        "" => Field::Any,
        "addition" => Field::Addition,
        "deletion" => Field::Deletion,
        "context" => Field::Context,
        "comment" => Field::Comment,
        "string" => Field::String,
        "identifier" => Field::Identifier,
        "path" => Field::Path,
        "message" => Field::Message,
        "author" => Field::Author,
        "hunk" => Field::Hunk,
        other => return Err(format!("Unknown search field: {other}")),
    };
    Ok(SearchTerm {
        field,
        value: value.to_lowercase(),
    })
}

#[derive(Default, Clone)]
struct LexerState {
    block_comment: bool,
    triple_quote: Option<char>,
    template_string: bool,
    template_escaped: bool,
}

#[derive(Default)]
struct LexedLine {
    comment: String,
    string: String,
    identifiers: Vec<String>,
}

fn lex_line(path: &str, source: &str, state: &mut LexerState) -> LexedLine {
    let ext = Path::new(path)
        .extension()
        .and_then(|v| v.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let python = matches!(ext.as_str(), "py" | "pyi" | "pyw");
    let mut out = LexedLine::default();
    let chars: Vec<char> = source.chars().collect();
    let mut i = 0;
    let mut triple_quote = state.triple_quote.take();
    let mut quote = None;
    let mut escaped = false;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        if state.block_comment {
            out.comment.push(c);
            if c == '*' && next == Some('/') {
                out.comment.push('/');
                i += 1;
                state.block_comment = false;
            }
        } else if let Some(delimiter) = triple_quote {
            out.string.push(c);
            if python
                && c == delimiter
                && next == Some(delimiter)
                && chars.get(i + 2) == Some(&delimiter)
            {
                out.string.push(delimiter);
                out.string.push(delimiter);
                i += 2;
                triple_quote = None;
            }
        } else if state.template_string {
            out.string.push(c);
            if state.template_escaped {
                state.template_escaped = false;
            } else if c == '\\' {
                state.template_escaped = true;
            } else if c == '`' {
                state.template_string = false;
            }
        } else if let Some(delimiter) = quote {
            out.string.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == delimiter {
                quote = None;
            }
        } else if c == '/' && next == Some('*') && !python {
            state.block_comment = true;
            i += 1;
        } else if c == '/' && next == Some('/') && !python {
            out.comment
                .push_str(&chars[i + 2..].iter().collect::<String>());
            break;
        } else if c == '#' && (python || matches!(ext.as_str(), "sh" | "bash" | "zsh" | "rb")) {
            out.comment
                .push_str(&chars[i + 1..].iter().collect::<String>());
            break;
        } else if c == '`' && matches!(ext.as_str(), "js" | "jsx" | "ts" | "tsx" | "mjs" | "cjs") {
            state.template_string = true;
        } else if c == '"' || c == '\'' {
            if python && next == Some(c) && chars.get(i + 2) == Some(&c) {
                triple_quote = Some(c);
                i += 2;
            } else {
                quote = Some(c);
            }
        } else if c.is_ascii_alphabetic() || c == '_' || c == '$' {
            let start = i;
            i += 1;
            while i < chars.len()
                && (chars[i].is_ascii_alphanumeric() || matches!(chars[i], '_' | '$'))
            {
                i += 1;
            }
            out.identifiers
                .push(chars[start..i].iter().collect::<String>().to_lowercase());
            i -= 1;
        }
        i += 1;
    }
    state.triple_quote = triple_quote;
    out
}

fn evaluate(expr: &Expr, index: &SearchIndex, line: &IndexLine, lexed: &LexedLine) -> bool {
    match expr {
        Expr::Term(term) => {
            let value = &term.value;
            let text = line.text.to_lowercase();
            let path = index.paths.get(line.path).map(String::as_str).unwrap_or("");
            match term.field {
                Field::Any => text.contains(value),
                Field::Addition => line.kind == LineKind::Addition && text.contains(value),
                Field::Deletion => line.kind == LineKind::Deletion && text.contains(value),
                Field::Context => line.kind == LineKind::Context && text.contains(value),
                Field::Comment => lexed.comment.to_lowercase().contains(value),
                Field::String => lexed.string.to_lowercase().contains(value),
                Field::Identifier => lexed
                    .identifiers
                    .iter()
                    .any(|identifier| identifier == value),
                Field::Path => path.to_lowercase().contains(value),
                Field::Message => index
                    .commits
                    .get(&line.commit)
                    .is_some_and(|meta| meta.message.to_lowercase().contains(value)),
                Field::Author => index
                    .commits
                    .get(&line.commit)
                    .is_some_and(|meta| meta.author.to_lowercase().contains(value)),
                Field::Hunk => line
                    .hunk
                    .and_then(|id| index.hunks.get(id))
                    .is_some_and(|hunk| hunk.to_lowercase().contains(value)),
            }
        }
        Expr::Not(inner) => !evaluate(inner, index, line, lexed),
        Expr::And(left, right) => {
            evaluate(left, index, line, lexed) && evaluate(right, index, line, lexed)
        }
        Expr::Or(left, right) => {
            evaluate(left, index, line, lexed) || evaluate(right, index, line, lexed)
        }
    }
}

#[derive(Clone, Debug)]
struct SearchResult {
    commit: String,
    message: String,
    path: String,
    line: u32,
    snippet: String,
}

fn search_index(index: &SearchIndex, query: &str) -> Result<Vec<SearchResult>, String> {
    let expr = parse_query(query)?;
    let mut results = Vec::new();
    let mut lexer = HashMap::<(String, usize, Option<usize>, bool), LexerState>::new();
    for line in &index.lines {
        let path = index.paths.get(line.path).map(String::as_str).unwrap_or("");
        let old_side = line.kind == LineKind::Deletion;
        let lexed = lex_line(
            path,
            &line.text,
            lexer
                .entry((line.commit.clone(), line.path, line.hunk, old_side))
                .or_default(),
        );
        if evaluate(&expr, index, line, &lexed) {
            results.push(SearchResult {
                commit: line.commit.clone(),
                message: index
                    .commits
                    .get(&line.commit)
                    .map(|meta| meta.message.clone())
                    .unwrap_or_default(),
                path: path.to_owned(),
                line: line.line,
                snippet: line.text.trim().to_owned(),
            });
            if results.len() >= MAX_RESULTS {
                break;
            }
        }
    }
    Ok(results)
}

fn truncate_utf8(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_owned();
    }
    let mut end = max_bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_owned()
}

fn absorb_worker_commands(
    commands: &Receiver<WorkerCommand>,
    pending_query: &mut Option<(u64, String)>,
) -> bool {
    loop {
        match commands.try_recv() {
            Ok(WorkerCommand::Cancel) | Err(TryRecvError::Disconnected) => return false,
            Ok(WorkerCommand::Search { sequence, query }) => {
                *pending_query = Some((sequence, query));
            }
            Err(TryRecvError::Empty) => return true,
        }
    }
}

type PendingQuery = Option<(u64, String)>;
type IndexBuildResult = Result<(SearchIndex, bool, PendingQuery), String>;

fn build_index(
    path: &Path,
    generation: u64,
    output: &Sender<WorkerEvent>,
    commands: &Receiver<WorkerCommand>,
) -> IndexBuildResult {
    let repo = Repository::open(path)
        .map_err(|error| format!("Could not open repository for search: {error}"))?;
    let mut walk = repo.revwalk().map_err(|error| error.message().to_owned())?;
    let mut tips = HashSet::new();
    if let Ok(references) = repo.references() {
        for reference in references.flatten() {
            if let Some(name) = reference.name()
                && (name.starts_with("refs/heads/")
                    || name.starts_with("refs/remotes/")
                    || name.starts_with("refs/tags/"))
                && let Ok(commit) = reference.peel_to_commit()
            {
                tips.insert(commit.id());
            }
        }
    }
    if let Ok(head) = repo.head().and_then(|head| head.peel_to_commit()) {
        tips.insert(head.id());
    }
    for tip in tips {
        walk.push(tip).map_err(|error| error.message().to_owned())?;
    }
    walk.set_sorting(git2::Sort::TOPOLOGICAL | git2::Sort::TIME)
        .map_err(|error| error.message().to_owned())?;
    let mut index = SearchIndex::default();
    let mut cancelled = false;
    let mut pending_query = None;
    for oid in walk {
        if !absorb_worker_commands(commands, &mut pending_query) {
            cancelled = true;
            break;
        }
        if index.commits_indexed >= MAX_COMMITS
            || index.lines.len() >= MAX_INDEX_LINES
            || index.bytes >= MAX_INDEX_BYTES
        {
            index.truncated = true;
            break;
        }
        let oid = oid.map_err(|error| error.message().to_owned())?;
        let commit = repo
            .find_commit(oid)
            .map_err(|error| error.message().to_owned())?;
        let commit_id = commit.id().to_string();
        let message = truncate_utf8(commit.message().unwrap_or(""), 4096);
        let author = truncate_utf8(commit.author().name().unwrap_or(""), 256);
        index
            .commits
            .insert(commit_id.clone(), CommitMetadata { message, author });
        let new_tree = commit.tree().map_err(|error| error.message().to_owned())?;
        let old_tree = commit.parent(0).ok().and_then(|parent| parent.tree().ok());
        let diff = repo
            .diff_tree_to_tree(old_tree.as_ref(), Some(&new_tree), None)
            .map_err(|error| error.message().to_owned())?;
        let mut current_hunk = None;
        let mut current_path = String::new();
        let mut current_path_id = 0usize;
        let mut line_count = 0usize;
        let mut byte_count = 0usize;
        let mut limit_hit = false;
        let mut cancelled_during_diff = false;
        let mut callback_count = 0usize;
        let print_result = diff.print(git2::DiffFormat::Patch, |delta, hunk, diff_line| {
            callback_count += 1;
            if callback_count.is_multiple_of(128)
                && !absorb_worker_commands(commands, &mut pending_query)
            {
                cancelled_during_diff = true;
                return false;
            }
            if let Some(path) = delta.new_file().path().or_else(|| delta.old_file().path()) {
                let path = path.to_string_lossy().into_owned();
                if path != current_path {
                    current_path = path;
                    current_path_id = if let Some(id) = index.path_ids.get(&current_path) {
                        *id
                    } else {
                        if index.bytes + byte_count + current_path.len() > MAX_INDEX_BYTES {
                            limit_hit = true;
                            return false;
                        }
                        let id = index.paths.len();
                        byte_count += current_path.len();
                        index.paths.push(current_path.clone());
                        index.path_ids.insert(current_path.clone(), id);
                        id
                    };
                    current_hunk = None;
                }
            }
            if let Some(hunk) = hunk {
                let key = (hunk.old_start(), hunk.new_start());
                if current_hunk != Some(key) {
                    current_hunk = Some(key);
                    index.hunks.push(String::new());
                }
            }
            let kind = match diff_line.origin() {
                '+' => LineKind::Addition,
                '-' => LineKind::Deletion,
                ' ' => LineKind::Context,
                _ => return true,
            };
            let content = diff_line.content();
            if content.len() > MAX_SINGLE_LINE_BYTES || index.lines.len() >= MAX_INDEX_LINES {
                limit_hit = true;
                return false;
            }
            // Invalid UTF-8 may expand to three bytes per source byte. Reserve six times
            // the patch bytes for the decoded line and its hunk-scope copy.
            byte_count += content.len().saturating_mul(6);
            if index.bytes + byte_count > MAX_INDEX_BYTES {
                limit_hit = true;
                return false;
            }
            let text = String::from_utf8_lossy(content)
                .trim_end_matches(['\r', '\n'])
                .to_owned();
            let hunk_id = current_hunk.map(|_| index.hunks.len() - 1);
            if let Some(id) = hunk_id {
                index.hunks[id].push_str(&text);
                index.hunks[id].push('\n');
            }
            let line = match kind {
                LineKind::Deletion => diff_line.old_lineno(),
                _ => diff_line.new_lineno(),
            }
            .unwrap_or(0);
            index.lines.push(IndexLine {
                commit: commit_id.clone(),
                path: current_path_id,
                line,
                kind,
                text,
                hunk: hunk_id,
            });
            line_count += 1;
            true
        });
        if !cancelled_during_diff && !limit_hit {
            print_result.map_err(|error| error.message().to_owned())?;
        }
        if cancelled_during_diff {
            cancelled = true;
            break;
        }
        index.bytes += byte_count;
        index.commits_indexed += 1;
        if limit_hit {
            index.truncated = true;
            break;
        }
        if index.commits_indexed % 40 == 0 {
            let _ = output.send(WorkerEvent::Progress {
                generation,
                commits: index.commits_indexed,
                lines: index.lines.len(),
                bytes: index.bytes,
            });
        }
        if line_count > 0
            && (index.lines.len() >= MAX_INDEX_LINES || index.bytes >= MAX_INDEX_BYTES)
        {
            index.truncated = true;
            break;
        }
    }
    Ok((index, cancelled, pending_query))
}

enum WorkerCommand {
    Search { sequence: u64, query: String },
    Cancel,
}
enum WorkerEvent {
    Progress {
        generation: u64,
        commits: usize,
        lines: usize,
        bytes: usize,
    },
    Ready {
        generation: u64,
        commits: usize,
        lines: usize,
        truncated: bool,
    },
    Results {
        generation: u64,
        sequence: u64,
        results: Vec<SearchResult>,
    },
    Error {
        generation: u64,
        message: String,
    },
}

struct WorkerHandle {
    sender: Sender<WorkerCommand>,
    receiver: Receiver<WorkerEvent>,
}

#[derive(Default)]
pub(crate) struct CodeSearchTools {
    query: String,
    query_sent: String,
    sequence: u64,
    generation: u64,
    repo_key: Option<String>,
    worker: Option<WorkerHandle>,
    ready: bool,
    progress: String,
    error: Option<String>,
    results: Vec<SearchResult>,
    truncated: bool,
}

impl CodeSearchTools {
    pub(crate) fn cancel_for_repo_change(&mut self, new_path: Option<&Path>) {
        let new_key = new_path.map(history_features::repo_storage_key);
        if self.repo_key != new_key {
            if let Some(worker) = self.worker.take() {
                let _ = worker.sender.send(WorkerCommand::Cancel);
            }
            self.generation = self.generation.wrapping_add(1);
            self.repo_key = new_key;
            self.ready = false;
            self.results.clear();
            self.error = None;
            self.progress.clear();
            self.query_sent.clear();
        }
    }

    fn start_build(&mut self, path: PathBuf) {
        if let Some(worker) = self.worker.take() {
            let _ = worker.sender.send(WorkerCommand::Cancel);
        }
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        let (command_tx, command_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        self.ready = false;
        self.results.clear();
        self.error = None;
        self.progress = "Building search index…".into();
        self.worker = Some(WorkerHandle {
            sender: command_tx,
            receiver: event_rx,
        });
        thread::spawn(
            move || match build_index(&path, generation, &event_tx, &command_rx) {
                Ok((_, true, _)) => (),
                Ok((index, false, initial_query)) => {
                    let _ = event_tx.send(WorkerEvent::Ready {
                        generation,
                        commits: index.commits_indexed,
                        lines: index.lines.len(),
                        truncated: index.truncated,
                    });
                    let mut pending = initial_query;
                    loop {
                        let (mut sequence, mut query) = if let Some(pending) = pending.take() {
                            pending
                        } else {
                            match command_rx.recv() {
                                Ok(WorkerCommand::Cancel) | Err(_) => break,
                                Ok(WorkerCommand::Search { sequence, query }) => (sequence, query),
                            }
                        };
                        while let Ok(command) = command_rx.try_recv() {
                            match command {
                                WorkerCommand::Cancel => return,
                                WorkerCommand::Search {
                                    sequence: new_sequence,
                                    query: new_query,
                                } => {
                                    sequence = new_sequence;
                                    query = new_query;
                                }
                            }
                        }
                        let results = search_index(&index, &query);
                        let message = match results {
                            Ok(results) => WorkerEvent::Results {
                                generation,
                                sequence,
                                results,
                            },
                            Err(message) => WorkerEvent::Error {
                                generation,
                                message,
                            },
                        };
                        if event_tx.send(message).is_err() {
                            break;
                        }
                    }
                }
                Err(message) => {
                    let _ = event_tx.send(WorkerEvent::Error {
                        generation,
                        message,
                    });
                }
            },
        );
    }

    fn search(&mut self) {
        if !self.ready || self.query.trim().is_empty() || self.query == self.query_sent {
            return;
        }
        self.sequence = self.sequence.wrapping_add(1);
        self.query_sent = self.query.clone();
        self.results.clear();
        if let Some(worker) = &self.worker {
            let _ = worker.sender.send(WorkerCommand::Search {
                sequence: self.sequence,
                query: self.query.clone(),
            });
        }
    }
}

impl GritApp {
    pub(crate) fn code_search_ui(&mut self, ui: &mut egui::Ui) {
        self.code_search
            .cancel_for_repo_change(self.path.as_deref());
        let events = self
            .code_search
            .worker
            .as_ref()
            .map(|worker| worker.receiver.try_iter().collect::<Vec<_>>())
            .unwrap_or_default();
        for event in events {
            match event {
                WorkerEvent::Progress {
                    generation,
                    commits,
                    lines,
                    bytes,
                } if generation == self.code_search.generation => {
                    self.code_search.progress = format!(
                        "Indexed {commits} commits, {lines} lines ({:.1} MiB)…",
                        bytes as f64 / (1024. * 1024.)
                    );
                }
                WorkerEvent::Ready {
                    generation,
                    commits,
                    lines,
                    truncated,
                } if generation == self.code_search.generation => {
                    self.code_search.ready = true;
                    self.code_search.truncated = truncated;
                    self.code_search.progress =
                        format!("Index ready: {commits} commits, {lines} diff lines.");
                }
                WorkerEvent::Results {
                    generation,
                    sequence,
                    results,
                } if generation == self.code_search.generation
                    && sequence == self.code_search.sequence =>
                {
                    self.code_search.results = results;
                    self.code_search.error = None;
                }
                WorkerEvent::Error {
                    generation,
                    message,
                } if generation == self.code_search.generation => {
                    self.code_search.error = Some(message)
                }
                _ => {}
            }
        }
        ui.heading("History code search");
        ui.label("Lexical heuristics for Rust, C/C++, JavaScript/TypeScript and Python; this is not a language parser.");
        ui.label("Lexical state resets at hunk boundaries and is separate for old and new diff sides; code outside visible hunks is unknown.");
        ui.label("Build or refresh the local index explicitly. It covers local/remote branches and tags without network access.");
        ui.horizontal(|ui| {
            let busy = self.operation.is_some();
            if ui
                .add_enabled(
                    self.path.is_some() && !busy,
                    egui::Button::new(if self.code_search.ready {
                        "Refresh index"
                    } else {
                        "Build index"
                    }),
                )
                .clicked()
                && let Some(path) = self.path.clone()
            {
                self.code_search.start_build(path);
            }
            ui.label(&self.code_search.progress);
        });
        if self.code_search.truncated {
            ui.colored_label(Color32::YELLOW, format!("Index limit reached. Indexed at most {MAX_COMMITS} commits, {MAX_INDEX_LINES} lines and {MAX_INDEX_BYTES} bytes; refresh after narrowing the repository if needed."));
        }
        ui.horizontal(|ui| {
            ui.add_sized(
                [ui.available_width() - 100., 26.],
                egui::TextEdit::singleline(&mut self.code_search.query)
                    .hint_text("addition:TODO AND path:src/ NOT comment:obsolete"),
            );
            if ui
                .add_enabled(self.code_search.ready, egui::Button::new("Search"))
                .clicked()
            {
                self.code_search.query_sent.clear();
            }
        });
        if self.code_search.query != self.code_search.query_sent {
            self.code_search.search();
        }
        ui.horizontal_wrapped(|ui| {
            ui.label("Fields:");
            for field in [
                "addition:",
                "deletion:",
                "context:",
                "comment:",
                "string:",
                "identifier:",
                "path:",
                "message:",
                "author:",
                "hunk:",
            ] {
                if ui.small_button(field).clicked() {
                    self.code_search.query.push_str(field);
                }
            }
            ui.label("Operators: AND OR NOT ( ) · quotes group phrases");
        });
        if let Some(error) = &self.code_search.error {
            ui.colored_label(Color32::RED, error);
        }
        let results = self.code_search.results.clone();
        if self.code_search.ready && !self.code_search.query.trim().is_empty() {
            ui.label(format!("{} matches", results.len()));
            egui::ScrollArea::vertical()
                .max_height(ui.available_height())
                .show(ui, |ui| {
                    for result in results {
                        let short = result.commit.get(..8).unwrap_or(&result.commit);
                        let label = format!(
                            "{short}  {}:{}  {}",
                            result.path, result.line, result.snippet
                        );
                        if ui
                            .selectable_label(false, label)
                            .on_hover_text(&result.message)
                            .clicked()
                        {
                            self.select_history_commit(result.commit.clone());
                            self.selected_commit_file = Some(result.path.clone());
                            self.request_commit_detail(false);
                            self.notice = format!("Opened {}:{}", result.path, result.line);
                            self.tab = 1;
                        }
                    }
                });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        process::Command,
        time::{SystemTime, UNIX_EPOCH},
    };

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let stamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!("grit-code-search-{stamp}"));
            fs::create_dir_all(path.join("src")).unwrap();
            let fixture = Self(path);
            fixture.git(&["init", "-q"]);
            fixture.git(&["config", "user.name", "Search Author"]);
            fixture.git(&["config", "user.email", "search@example.test"]);
            fs::write(fixture.0.join("src/main.rs"), "fn top() {\n    let phrase = \"needle_string stringIdentifier\";\n    // needle_comment commentIdentifier\n    let identifierToken = phrase;\n    let contextNeedle = 1;\n    let removedNeedle = 2;\n    let replacedValue = 1;\n}\n").unwrap();
            fixture.git(&["add", "src/main.rs"]);
            fixture.git(&["commit", "-qm", "fixture initial"]);
            fs::write(fixture.0.join("src/main.rs"), "fn top() {\n    let phrase = \"needle_string stringIdentifier\";\n    // needle_comment commentIdentifier\n    let identifierToken = phrase;\n    let contextNeedle = 1;\n    let replacedValue = 3;\n}\n").unwrap();
            fixture.git(&["commit", "-qam", "fixture replacement"]);
            fixture
        }
        fn git(&self, args: &[&str]) -> String {
            let out = Command::new("git")
                .arg("-C")
                .arg(&self.0)
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
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn parser_obeys_not_and_and_or_precedence_and_grouping() {
        let parsed = parse_query("addition:foo OR deletion:bar AND NOT path:vendor").unwrap();
        assert!(matches!(parsed, Expr::Or(_, right) if matches!(*right, Expr::And(_, _))));
        assert!(matches!(
            parse_query("message:\"quoted phrase\" AND (path:src OR author:alice)"),
            Ok(Expr::And(_, _))
        ));
        assert!(parse_query("a OR (b AND c)").is_ok());
        assert_eq!(
            parse_query("\"AND\" AND message:\"OR\"").unwrap(),
            Expr::And(
                Box::new(Expr::Term(SearchTerm {
                    field: Field::Any,
                    value: "and".into(),
                })),
                Box::new(Expr::Term(SearchTerm {
                    field: Field::Message,
                    value: "or".into(),
                })),
            )
        );
    }

    #[test]
    fn parser_rejects_unmatched_quotes_operators_parentheses_and_unknown_fields() {
        for query in [
            "message:\"unterminated",
            "AND a",
            "a OR",
            "(a OR b",
            "a)",
            "unknown:value",
        ] {
            assert!(parse_query(query).is_err(), "accepted {query:?}");
        }
    }

    fn fixture_index(fixture: &Fixture) -> SearchIndex {
        let (event_tx, _event_rx) = mpsc::channel();
        let (_command_tx, command_rx) = mpsc::channel();
        let (index, cancelled, pending) =
            build_index(&fixture.0, 7, &event_tx, &command_rx).unwrap();
        assert!(!cancelled);
        assert!(pending.is_none());
        index
    }

    #[test]
    fn real_history_diff_indexes_change_kinds_context_hunks_and_lexical_fields() {
        let fixture = Fixture::new();
        let index = fixture_index(&fixture);
        assert!(
            !search_index(&index, "addition:replacedValue")
                .unwrap()
                .is_empty()
        );
        assert!(
            !search_index(&index, "deletion:removedNeedle")
                .unwrap()
                .is_empty()
        );
        assert!(
            !search_index(&index, "context:contextNeedle")
                .unwrap()
                .is_empty()
        );
        assert!(
            !search_index(&index, "comment:needle_comment")
                .unwrap()
                .is_empty()
        );
        assert!(
            !search_index(&index, "string:needle_string")
                .unwrap()
                .is_empty()
        );
        assert!(
            !search_index(&index, "identifier:identifiertoken")
                .unwrap()
                .is_empty()
        );
        assert!(
            search_index(&index, "comment:identifierToken")
                .unwrap()
                .is_empty()
        );
        assert!(
            search_index(&index, "string:identifierToken")
                .unwrap()
                .is_empty()
        );
        assert!(
            search_index(&index, "identifier:needle_string")
                .unwrap()
                .is_empty()
        );
        assert!(
            search_index(&index, "identifier:stringIdentifier")
                .unwrap()
                .is_empty()
        );
        assert!(
            search_index(&index, "identifier:commentIdentifier")
                .unwrap()
                .is_empty()
        );
        assert!(
            !search_index(&index, "hunk:removedNeedle")
                .unwrap()
                .is_empty()
        );
        assert!(
            !search_index(
                &index,
                "path:src/main.rs AND message:replacement AND author:\"search author\""
            )
            .unwrap()
            .is_empty()
        );
    }

    #[test]
    fn lexer_carries_multiline_comment_and_python_triple_string_state() {
        let mut state = LexerState::default();
        let first = lex_line("src/sample.rs", "/* hiddenIdentifier", &mut state);
        assert!(first.identifiers.is_empty());
        let second = lex_line(
            "src/sample.rs",
            " stillHidden */ visibleIdentifier()",
            &mut state,
        );
        assert!(!second.identifiers.contains(&"stillhidden".into()));
        assert!(second.identifiers.contains(&"visibleidentifier".into()));

        let mut state = LexerState::default();
        let first = lex_line("src/sample.py", "value = \"\"\"stringOnly", &mut state);
        assert!(first.identifiers.contains(&"value".into()));
        let second = lex_line(
            "src/sample.py",
            "hiddenIdentifier\"\"\"; visibleIdentifier = 1",
            &mut state,
        );
        assert!(!second.identifiers.contains(&"hiddenidentifier".into()));
        assert!(second.identifiers.contains(&"visibleidentifier".into()));
    }

    #[test]
    fn hash_starts_a_comment_only_for_python_like_sources() {
        let python = lex_line(
            "src/module.py",
            "value = 1 # hiddenIdentifier",
            &mut LexerState::default(),
        );
        assert!(python.comment.contains("hiddenIdentifier"));
        assert!(!python.identifiers.contains(&"hiddenidentifier".into()));

        let javascript = lex_line(
            "src/module.js",
            "const value = 1; # hashIdentifier",
            &mut LexerState::default(),
        );
        assert!(javascript.comment.is_empty());
        assert!(javascript.identifiers.contains(&"hashidentifier".into()));
    }

    #[test]
    fn build_preserves_latest_search_and_honors_cancellation() {
        let fixture = Fixture::new();
        let (event_tx, _event_rx) = mpsc::channel();
        let (command_tx, command_rx) = mpsc::channel();
        command_tx
            .send(WorkerCommand::Search {
                sequence: 1,
                query: "old".into(),
            })
            .unwrap();
        command_tx
            .send(WorkerCommand::Search {
                sequence: 2,
                query: "new".into(),
            })
            .unwrap();
        let (_, cancelled, pending) = build_index(&fixture.0, 11, &event_tx, &command_rx).unwrap();
        assert!(!cancelled);
        assert_eq!(pending, Some((2, "new".into())));

        let (cancel_tx, cancel_rx) = mpsc::channel();
        cancel_tx.send(WorkerCommand::Cancel).unwrap();
        let (index, cancelled, _) = build_index(&fixture.0, 12, &event_tx, &cancel_rx).unwrap();
        assert!(cancelled);
        assert_eq!(index.commits_indexed, 0);
    }

    #[test]
    fn oversized_diff_line_is_rejected_before_utf8_conversion() {
        let fixture = Fixture::new();
        let huge = format!("{}\n", "x".repeat(MAX_SINGLE_LINE_BYTES + 1));
        fs::write(fixture.0.join("src/huge.rs"), huge).unwrap();
        fixture.git(&["add", "src/huge.rs"]);
        fixture.git(&["commit", "-qm", "oversized line"]);
        let index = fixture_index(&fixture);
        assert!(index.truncated);
        assert!(index.lines.is_empty());
    }

    #[test]
    fn index_includes_local_remote_and_tag_tips_beyond_head() {
        let fixture = Fixture::new();
        let main = fixture.git(&["branch", "--show-current"]);
        let previous = fixture.git(&["rev-parse", "HEAD~1"]);
        fixture.git(&["switch", "-q", "-c", "side-only", &previous]);
        fs::write(
            fixture.0.join("src/branch.rs"),
            "fn branchOnlyNeedle() {}\n",
        )
        .unwrap();
        fixture.git(&["add", "src/branch.rs"]);
        fixture.git(&["commit", "-qm", "branch only commit"]);
        let branch_oid = fixture.git(&["rev-parse", "HEAD"]);
        fixture.git(&["update-ref", "refs/remotes/origin/topic", &branch_oid]);
        fixture.git(&["tag", "branch-archive", &branch_oid]);
        fixture.git(&["switch", "-q", &main]);

        let index = fixture_index(&fixture);
        assert!(
            !search_index(&index, "addition:branchOnlyNeedle")
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn unborn_repository_builds_an_empty_index_without_error() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("grit-code-search-unborn-{stamp}"));
        fs::create_dir_all(&path).unwrap();
        let out = Command::new("git")
            .arg("-C")
            .arg(&path)
            .args(["init", "-q"])
            .output()
            .unwrap();
        assert!(out.status.success());
        let (event_tx, _event_rx) = mpsc::channel();
        let (_command_tx, command_rx) = mpsc::channel();
        let result = build_index(&path, 13, &event_tx, &command_rx);
        let _ = fs::remove_dir_all(&path);
        let (index, cancelled, pending) = result.unwrap();
        assert!(!cancelled);
        assert!(pending.is_none());
        assert_eq!(index.commits_indexed, 0);
    }

    #[test]
    fn changing_repository_cancels_worker_and_advances_generation() {
        let first = Path::new("/tmp/grit-search-repo-a");
        let second = Path::new("/tmp/grit-search-repo-b");
        let (command_tx, command_rx) = mpsc::channel();
        let (_event_tx, event_rx) = mpsc::channel();
        let mut tools = CodeSearchTools {
            generation: 4,
            repo_key: Some(history_features::repo_storage_key(first)),
            worker: Some(WorkerHandle {
                sender: command_tx,
                receiver: event_rx,
            }),
            ..Default::default()
        };
        tools.cancel_for_repo_change(Some(second));
        assert_eq!(tools.generation, 5);
        assert_eq!(
            tools.repo_key,
            Some(history_features::repo_storage_key(second))
        );
        assert!(matches!(command_rx.try_recv(), Ok(WorkerCommand::Cancel)));
    }
}
