use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    Pick,
    Reword,
    Edit,
    Squash,
    Fixup,
    FixupLowerC,
    FixupUpperC,
    Drop,
}

impl Action {
    const ALL: [Self; 8] = [
        Self::Pick,
        Self::Reword,
        Self::Edit,
        Self::Squash,
        Self::Fixup,
        Self::FixupLowerC,
        Self::FixupUpperC,
        Self::Drop,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Pick => "pick",
            Self::Reword => "reword",
            Self::Edit => "edit",
            Self::Squash => "squash",
            Self::Fixup => "fixup",
            Self::FixupLowerC => "fixup -c",
            Self::FixupUpperC => "fixup -C",
            Self::Drop => "drop",
        }
    }

    fn is_fixup(self) -> bool {
        matches!(self, Self::Fixup | Self::FixupLowerC | Self::FixupUpperC)
    }
}

#[derive(Clone, Debug)]
struct CommitTodo {
    action: Action,
    prefix: String,
    oid: String,
    tail: String,
}

#[derive(Clone, Debug)]
enum TodoKind {
    Commit(CommitTodo),
    ReadOnly,
}

#[derive(Clone, Debug)]
struct TodoLine {
    content: String,
    has_newline: bool,
    kind: TodoKind,
}

impl TodoLine {
    fn parse(content: String, has_newline: bool) -> Self {
        let kind = parse_commit_line(&content)
            .map(TodoKind::Commit)
            .unwrap_or(TodoKind::ReadOnly);
        Self {
            content,
            has_newline,
            kind,
        }
    }

    fn action(&self) -> Option<Action> {
        match &self.kind {
            TodoKind::Commit(commit) => Some(commit.action),
            TodoKind::ReadOnly => None,
        }
    }

    fn set_action(&mut self, action: Action) {
        let TodoKind::Commit(commit) = &mut self.kind else {
            return;
        };
        if commit.action == action {
            return;
        }
        commit.action = action;
        let (verb, mode) = match action {
            Action::FixupLowerC => ("fixup", Some("-c")),
            Action::FixupUpperC => ("fixup", Some("-C")),
            other => (other.label(), None),
        };
        let mut content = format!("{}{}", commit.prefix, verb);
        if let Some(mode) = mode {
            content.push(' ');
            content.push_str(mode);
        }
        content.push(' ');
        content.push_str(&commit.oid);
        content.push_str(&commit.tail);
        self.content = content;
    }
}

fn token_spans(line: &str) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut start = None;
    for (index, character) in line.char_indices() {
        if character.is_whitespace() {
            if let Some(begin) = start.take() {
                spans.push((begin, index));
            }
        } else if start.is_none() {
            start = Some(index);
        }
    }
    if let Some(begin) = start {
        spans.push((begin, line.len()));
    }
    spans
}

fn parse_commit_line(line: &str) -> Option<CommitTodo> {
    let spans = token_spans(line);
    let (action_start, action_end) = *spans.first()?;
    let raw_action = &line[action_start..action_end];
    let mut action = match raw_action {
        "pick" | "p" => Action::Pick,
        "reword" | "r" => Action::Reword,
        "edit" | "e" => Action::Edit,
        "squash" | "s" => Action::Squash,
        "fixup" | "f" => Action::Fixup,
        "drop" | "d" => Action::Drop,
        _ => return None,
    };
    let mut oid_index = 1;
    if raw_action == "fixup"
        && spans
            .get(1)
            .is_some_and(|(start, end)| matches!(&line[*start..*end], "-c" | "-C"))
    {
        action = if &line[spans[1].0..spans[1].1] == "-c" {
            Action::FixupLowerC
        } else {
            Action::FixupUpperC
        };
        oid_index = 2;
    }
    let (oid_start, oid_end) = *spans.get(oid_index)?;
    let oid = &line[oid_start..oid_end];
    if !(4..=64).contains(&oid.len()) || !oid.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let (prefix, tail) = (&line[..action_start], &line[oid_end..]);
    Some(CommitTodo {
        action,
        prefix: prefix.to_owned(),
        oid: oid.to_owned(),
        tail: tail.to_owned(),
    })
}

fn parse_plan(plan: &str) -> Vec<TodoLine> {
    let mut lines = Vec::new();
    for part in plan.split_inclusive('\n') {
        let has_newline = part.ends_with('\n');
        let content = if has_newline {
            &part[..part.len() - 1]
        } else {
            part
        };
        lines.push(TodoLine::parse(content.to_owned(), has_newline));
    }
    lines
}

fn serialize_plan(lines: &[TodoLine]) -> String {
    let mut result = String::new();
    for line in lines {
        result.push_str(&line.content);
        if line.has_newline {
            result.push('\n');
        }
    }
    result
}

fn validate_lines(lines: &[TodoLine]) -> Result<(), String> {
    let mut has_kept_commit = false;
    let mut commit_number = 0;
    for line in lines {
        let Some(action) = line.action() else {
            continue;
        };
        commit_number += 1;
        match action {
            Action::Squash if !has_kept_commit => {
                return Err(format!(
                    "Commit line {commit_number}: squash needs an earlier kept commit"
                ));
            }
            action if action.is_fixup() && !has_kept_commit => {
                return Err(format!(
                    "Commit line {commit_number}: fixup needs an earlier kept commit"
                ));
            }
            Action::Pick | Action::Reword | Action::Edit => has_kept_commit = true,
            Action::Squash
            | Action::Fixup
            | Action::FixupLowerC
            | Action::FixupUpperC
            | Action::Drop => {}
        }
    }
    if commit_number == 0 {
        return Err("The rebase plan must contain at least one supported commit action".into());
    }
    Ok(())
}

pub(super) fn validate_plan(plan: &str) -> Result<(), String> {
    validate_lines(&parse_plan(plan))
}

fn display_commit(commit: &CommitTodo) -> (String, String) {
    let abbreviated = if commit.oid.len() > 12 {
        format!("{}…", &commit.oid[..12])
    } else {
        commit.oid.clone()
    };
    let subject = commit.tail.trim_start().trim_end_matches('\r');
    let visible = if subject.chars().count() > 70 {
        format!("{}…", subject.chars().take(70).collect::<String>())
    } else {
        subject.to_owned()
    };
    (abbreviated, visible)
}

/// Show a safe graphical editor for supported commit commands. Lines Gitty does not
/// understand remain read-only and round-trip byte-for-byte, including line endings.
pub(super) fn rebase_plan_ui(ui: &mut egui::Ui, plan: &mut String) -> Result<(), String> {
    let mut lines = parse_plan(plan);
    let mut move_line = None;
    for index in 0..lines.len() {
        let initial_action = lines[index].action();
        let commit = match &lines[index].kind {
            TodoKind::Commit(commit) => Some(commit.clone()),
            TodoKind::ReadOnly => None,
        };
        if let Some(commit) = commit {
            let mut selected = commit.action;
            let mut dropped = selected == Action::Drop;
            let mut checkbox_changed = false;
            let mut moved = None;
            let (oid, subject) = display_commit(&commit);
            let shorter = lines.len();
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt(("rebase_todo_action", index))
                    .selected_text(selected.label())
                    .show_ui(ui, |ui| {
                        for action in Action::ALL {
                            ui.selectable_value(&mut selected, action, action.label());
                        }
                    });
                checkbox_changed = ui.checkbox(&mut dropped, "drop").changed();
                ui.label(RichText::new(oid).monospace())
                    .on_hover_text(&commit.oid);
                ui.label(subject).on_hover_text(commit.tail.trim());
                if ui.add_enabled(index > 0, egui::Button::new("↑")).clicked() {
                    moved = Some((index, index - 1));
                }
                if ui
                    .add_enabled(index + 1 < shorter, egui::Button::new("↓"))
                    .clicked()
                {
                    moved = Some((index, index + 1));
                }
            });
            if checkbox_changed {
                lines[index].set_action(if dropped { Action::Drop } else { Action::Pick });
            } else if selected != initial_action.unwrap_or(selected) {
                lines[index].set_action(selected);
            }
            if moved.is_some() {
                move_line = moved;
            }
        } else {
            let line = &lines[index].content;
            let display = if line.is_empty() {
                "(blank line)"
            } else {
                line
            };
            ui.horizontal(|ui| {
                ui.label(RichText::new(display).monospace().weak())
                    .on_hover_text("Preserved as written; advanced Git todo commands and comments are read-only here.");
            });
        }
    }
    if let Some((from, to)) = move_line {
        lines.swap(from, to);
    }
    *plan = serialize_plan(&lines);
    match validate_plan(&serialize_plan(&lines)) {
        Ok(()) => Ok(()),
        Err(error) => {
            ui.colored_label(Color32::LIGHT_RED, &error);
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_comments_and_advanced_commands_exact_while_reordering_and_changing_actions() {
        let original = "# keep this comment\r\npick abcdef0123456789 First subject\nexec echo stays raw\npick 1234567890abcdef A very long commit subject that remains unchanged\nfixup -c fedcba0987654321 Message editor\n";
        assert_eq!(serialize_plan(&parse_plan(original)), original);
        let mut lines = parse_plan(original);
        lines[1].set_action(Action::Reword);
        lines[4].set_action(Action::FixupUpperC);
        lines.swap(1, 3);
        let rebuilt = serialize_plan(&lines);
        assert!(rebuilt.starts_with("# keep this comment\r\n"));
        assert!(rebuilt.contains("exec echo stays raw\n"));
        assert!(rebuilt.contains("fixup -C fedcba0987654321 Message editor\n"));
        assert!(rebuilt.contains("reword abcdef0123456789 First subject\n"));
        assert_eq!(lines[0].content, "# keep this comment\r");
        assert!(validate_lines(&lines).is_ok());
    }

    #[test]
    fn first_squash_and_all_fixup_modes_require_a_kept_anchor() {
        assert!(validate_plan("squash abcdef0123456789 subject").is_err());
        assert!(validate_plan("fixup -c abcdef0123456789 subject").is_err());
        assert!(validate_plan("fixup -C abcdef0123456789 subject").is_err());
        for action in [
            Action::Squash,
            Action::Fixup,
            Action::FixupLowerC,
            Action::FixupUpperC,
        ] {
            let mut line = TodoLine::parse("pick abcdef0123456789 subject".into(), false);
            line.set_action(action);
            assert!(
                validate_lines(&[line]).is_err(),
                "{} must not be the first kept commit",
                action.label()
            );
        }
        let plan =
            parse_plan("drop abcdef0123456789 discarded\nfixup -c 0123456789abcdef still first\n");
        assert!(validate_lines(&plan).unwrap_err().contains("fixup"));
        let plan = parse_plan("pick abcdef0123456789 anchor\nfixup -C 0123456789abcdef joined\n");
        assert!(validate_lines(&plan).is_ok());
    }

    #[test]
    fn unsupported_action_forms_round_trip_without_interpretation() {
        for raw in [
            "merge -C abcdef0123456789 branch-label # message",
            "label branch-label",
            "reset onto",
            "update-ref refs/heads/topic",
            "noop",
            "break",
            "exec echo hello",
            "pick not-an-object subject",
        ] {
            let parsed = parse_plan(raw);
            assert_eq!(serialize_plan(&parsed), raw);
            assert!(matches!(parsed[0].kind, TodoKind::ReadOnly), "{raw}");
        }
    }
}
