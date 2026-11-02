use super::*;

pub(super) fn configure_style(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();
    style.visuals = egui::Visuals::dark();
    style.visuals.panel_fill = PANEL;
    style.visuals.window_fill = PANEL2;
    style.visuals.extreme_bg_color = BG;
    style.visuals.faint_bg_color = PANEL2;
    style.visuals.override_text_color = Some(TEXT);
    style.visuals.selection.bg_fill = Color32::from_rgb(44, 77, 108);
    style.visuals.selection.stroke = Stroke::new(1., BLUE);
    style.visuals.widgets.noninteractive.bg_stroke = Stroke::new(1., BORDER);
    style.visuals.widgets.inactive.weak_bg_fill = PANEL2;
    style.visuals.widgets.inactive.bg_fill = PANEL2;
    style.visuals.widgets.inactive.bg_stroke = Stroke::new(1., BORDER);
    style.visuals.widgets.hovered.weak_bg_fill = Color32::from_rgb(49, 57, 69);
    style.spacing.item_spacing = egui::vec2(6., 4.);
    style.spacing.button_padding = egui::vec2(10., 5.);
    style.spacing.interact_size.y = 26.;
    style
        .text_styles
        .insert(egui::TextStyle::Body, FontId::proportional(13.));
    style
        .text_styles
        .insert(egui::TextStyle::Button, FontId::proportional(13.));
    ctx.set_style(style);
}

impl GritApp {
    pub(super) fn set_history_scope(&mut self, name: Option<String>) {
        self.history_scope = name.map(|name| {
            let mut ids = HashSet::new();
            if let Some(repo) = self.repo()
                && let Ok(object) = repo.revparse_single(&name)
                && let Ok(mut walk) = repo.revwalk()
                && walk.push(object.id()).is_ok()
            {
                ids.extend(walk.filter_map(Result::ok).map(|id| id.to_string()));
            }
            (name, ids)
        });
    }

    pub(super) fn select_history_commit(&mut self, id: String) {
        let opts = &mut self.history_options;
        if opts.navigation.get(opts.navigation_index) != Some(&id) {
            if !opts.navigation.is_empty() {
                opts.navigation.truncate(opts.navigation_index + 1);
            }
            opts.navigation.push(id.clone());
            if opts.navigation.len() > 100 {
                opts.navigation.remove(0);
            }
            opts.navigation_index = opts.navigation.len() - 1;
        }
        self.load_history_commit(id);
    }
    pub(super) fn load_history_commit(&mut self, id: String) {
        self.selected_diff_lines.clear();
        self.selected_commit = Some(id.clone());
        self.selected_commit_file = None;
        if let Some(repo) = self.repo() {
            self.commit_files = commit_files(&repo, &id);
            self.diff = diff_commit_path(&repo, &id, None);
        }
    }

    pub(super) fn change_list(&mut self, ui: &mut egui::Ui, staged: bool, height: f32) {
        let changes: Vec<_> = self
            .changes
            .iter()
            .filter(|c| {
                c.staged == staged
                    && (!self.history_options.hide_untracked
                        || self.repo().is_none_or(|r| {
                            !r.status_file(Path::new(&c.path))
                                .is_ok_and(|s| s.contains(Status::WT_NEW))
                        }))
            })
            .cloned()
            .collect();
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!(
                    "{}   {}",
                    if staged {
                        "Staged changes"
                    } else {
                        "Unstaged changes"
                    },
                    changes.len()
                ))
                .strong()
                .color(if staged { GREEN } else { TEXT }),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui
                    .add_enabled(
                        !changes.is_empty() && self.operation.is_none(),
                        egui::Button::new(if staged { "Unstage all" } else { "Stage all" }).small(),
                    )
                    .clicked()
                {
                    for c in &changes {
                        self.stage(&c.path, staged);
                    }
                }
            });
        });
        if self.tree_mode {
            let selected = self
                .selected_file
                .clone()
                .filter(|p| self.selected_staged == staged && changes.iter().any(|c| &c.path == p));
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add_enabled(
                        selected.is_some(),
                        egui::Button::new(if staged {
                            "Unstage selected file"
                        } else {
                            "Stage selected file"
                        })
                        .small(),
                    )
                    .clicked()
                    && let Some(path) = &selected
                {
                    self.stage(path, staged);
                }
                if !staged
                    && ui
                        .add_enabled(selected.is_some(), egui::Button::new("Discard…").small())
                        .clicked()
                {
                    self.pending_discard = selected;
                }
            });
        }
        let height = if self.tree_mode {
            (height - 28.).max(50.)
        } else {
            height
        };
        egui::ScrollArea::vertical()
            .id_salt(("change_list", staged))
            .auto_shrink([false, false])
            .max_height(height.max(70.))
            .min_scrolled_height(height.max(70.))
            .show(ui, |ui| {
                if changes.is_empty() {
                    ui.label(
                        RichText::new(if staged {
                            "Stage changes to include them in your commit."
                        } else {
                            "Working tree clean"
                        })
                        .small()
                        .color(MUTED),
                    );
                }
                if self.tree_mode {
                    let paths = changes.iter().map(|c| c.path.clone()).collect::<Vec<_>>();
                    let statuses = changes.iter().map(|c| (c.path.clone(), c.status)).collect();
                    let selected = if self.selected_staged == staged {
                        self.selected_file.as_deref()
                    } else {
                        None
                    };
                    let action = tree_features::file_tree_status_ui(
                        ui,
                        (self.path.clone(), "changes", staged),
                        &paths,
                        selected,
                        &statuses,
                    );
                    match action {
                        Some(tree_features::TreeAction::Select(path)) => {
                            self.selected_diff_lines.clear();
                            self.selected_staged = staged;
                            self.select_file(path);
                        }
                        Some(tree_features::TreeAction::FileHistory(path)) => {
                            self.open_file_history(path)
                        }
                        Some(tree_features::TreeAction::CopyPath(path)) => ui.ctx().copy_text(path),
                        None => {}
                    }
                    return;
                }
                for c in changes {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(c.status).monospace().color(if staged {
                            GREEN
                        } else {
                            BLUE
                        }));
                        let width = (ui.available_width() - 42.).max(60.);
                        let response = ui.add_sized(
                            [width, 25.],
                            egui::Button::new(&c.path)
                                .selected(
                                    self.selected_file.as_deref() == Some(&c.path)
                                        && self.selected_staged == staged,
                                )
                                .frame(false)
                                .truncate(),
                        );
                        if response.clicked() {
                            self.selected_diff_lines.clear();
                            self.selected_staged = staged;
                            self.select_file(c.path.clone());
                        }
                        response.on_hover_text(&c.path).context_menu(|ui| {
                            if ui.button("File history…").clicked() {
                                self.open_file_history(c.path.clone());
                                ui.close_menu();
                            }
                            if ui.button("Copy path").clicked() {
                                ui.ctx().copy_text(c.path.clone());
                                ui.close_menu();
                            }

                            if ui
                                .button(if staged { "Unstage" } else { "Stage" })
                                .clicked()
                            {
                                self.stage(&c.path, staged);
                                ui.close_menu();
                            }
                            if !staged && ui.button("Discard changes…").clicked() {
                                self.pending_discard = Some(c.path.clone());
                                ui.close_menu();
                            }
                        });
                        if ui
                            .small_button(if staged { "−" } else { "+" })
                            .on_hover_text(if staged { "Unstage file" } else { "Stage file" })
                            .clicked()
                        {
                            self.stage(&c.path, staged);
                        }
                    });
                }
            });
    }

    pub(super) fn commit_editor(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.);
        ui.horizontal(|ui| {
            ui.label(RichText::new("Commit message").strong());
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.menu_button("More", |ui| {
                    if ui.button("Templates and author…").clicked() {
                        self.show_commit_options = true;
                        ui.close_menu();
                    }
                    ui.checkbox(
                        &mut self.history_options.hide_untracked,
                        "Hide untracked files",
                    );
                    if ui.button("Stash changes").clicked() {
                        self.git_action("Stash", &["stash", "push", "-u", "-m", "Saved from grit"]);
                        ui.close_menu();
                    }
                    if ui.button("Pop latest stash").clicked() {
                        self.git_action("Stash pop", &["stash", "pop"]);
                        ui.close_menu();
                    }
                });
            });
        });
        let height = (ui.available_height() - 67.).max(45.);
        ui.add_sized(
            [ui.available_width(), height],
            egui::TextEdit::multiline(&mut self.message)
                .hint_text("Summary of changes\n\nOptional description…")
                .desired_width(f32::INFINITY),
        );
        ui.horizontal(|ui| {
            if !self.commits.is_empty()
                && ui.checkbox(&mut self.amend, "Amend").changed()
                && self.amend
                && self.message.trim().is_empty()
                && let Some(repo) = self.repo()
            {
                match commit_features::load_amend_message(&repo) {
                    Ok(message) => self.message = message,
                    Err(error) => self.notice = error,
                }
            }
            let staged = self.changes.iter().filter(|c| c.staged).count();
            ui.label(
                RichText::new(format!("{staged} files staged"))
                    .small()
                    .color(MUTED),
            );
        });
        let can_commit = self.operation.is_none()
            && !self.message.trim().is_empty()
            && (self.amend || self.changes.iter().any(|c| c.staged));
        if ui
            .add_enabled(
                can_commit,
                egui::Button::new("Commit")
                    .min_size(egui::vec2(ui.available_width(), 28.))
                    .fill(Color32::from_rgb(43, 94, 137)),
            )
            .on_hover_text(if can_commit {
                "Create commit from staged changes"
            } else {
                "Stage files and enter a commit message"
            })
            .clicked()
        {
            self.commit();
        }
    }

    pub(super) fn staging_controls(&mut self, ui: &mut egui::Ui) {
        if !self.selected_staged
            && let (Some(path), Some(repo)) = (self.selected_file.clone(), self.repo())
        {
            let hunks = unstaged_hunks(&repo, &path);
            if !hunks.is_empty() {
                ui.horizontal_wrapped(|ui| {
                    if ui.button("Stage file").clicked() {
                        self.stage(&path, false);
                    }
                    ui.menu_button("Stage hunk", |ui| {
                        for (i, (old, new, _, _)) in hunks.iter().enumerate() {
                            if ui.button(format!("−{old} / +{new}")).clicked() {
                                self.stage_hunk(&path, i);
                                ui.close_menu();
                            }
                        }
                    });
                    if ui.button("Edit patch…").clicked() {
                        self.edit_stage_patch(&path);
                    }
                });
                ui.separator();
            }
        }
        self.patch_editor(ui);
    }

    pub(super) fn diff_panel(&mut self, ui: &mut egui::Ui, max_height: f32) {
        ui.horizontal_wrapped(|ui| {
            ui.checkbox(&mut self.wrap_diff, "Wrap lines");
            self.diff_preferences_ui(ui);
        });
        self.ensure_display_diff();
        if let Some(error) = &self.display_diff_error {
            ui.colored_label(Color32::LIGHT_RED, error);
            return;
        }
        let interactive =
            self.tab == 0 && self.operation.is_none() && !self.diff_preferences.ignore_whitespace;
        if self.tab == 0 && self.diff_preferences.ignore_whitespace {
            ui.label(
                RichText::new(
                    "Show whitespace to select individual lines. File staging remains available.",
                )
                .small()
                .color(MUTED),
            );
        }
        let selectable = if interactive {
            diff_features::selectable_lines(&self.display_diff)
        } else {
            HashSet::new()
        };
        if interactive {
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add_enabled(
                        !self.selected_diff_lines.is_empty(),
                        egui::Button::new(if self.selected_staged {
                            "Unstage selected lines (U)"
                        } else {
                            "Stage selected lines (S)"
                        }),
                    )
                    .clicked()
                {
                    self.apply_selected_lines();
                }
                if ui.small_button("Select all lines").clicked() {
                    self.selected_diff_lines = selectable.clone();
                }
                if ui.small_button("Clear").clicked() {
                    self.selected_diff_lines.clear();
                }
                ui.label(
                    RichText::new(format!("{} selected", self.selected_diff_lines.len()))
                        .small()
                        .color(MUTED),
                );
            });
            if !ui.ctx().wants_keyboard_input()
                && ui.ctx().input_mut(|input| {
                    input.consume_key(
                        egui::Modifiers::NONE,
                        if self.selected_staged {
                            egui::Key::U
                        } else {
                            egui::Key::S
                        },
                    )
                })
                && !self.selected_diff_lines.is_empty()
            {
                self.apply_selected_lines();
            }
        }
        if !self.load_large_diff
            && (self.display_diff.len() > 1024 * 1024 || self.display_diff.lines().count() > 10000)
        {
            ui.label("This is a large diff. Load it explicitly to review its text.");
            if ui.button("Load full diff").clicked() {
                self.load_large_diff = true;
            }
            return;
        }
        let patch = self.display_diff.clone();
        egui::ScrollArea::both()
            .id_salt("diff_content")
            .auto_shrink([false, false])
            .max_height((max_height - 60.).max(70.))
            .show(ui, |ui| {
                if patch.is_empty() {
                    ui.label(RichText::new("No text changes to display").color(MUTED));
                    return;
                }
                ui.spacing_mut().item_spacing.y = 0.;
                let mut old_line = 0usize;
                let mut new_line = 0usize;
                let mut in_hunk = false;
                for (i, line) in patch.lines().enumerate() {
                    if line.starts_with("diff --git ") {
                        in_hunk = false;
                    }
                    let header = !in_hunk || line.starts_with("@@") || line.starts_with('\\');
                    if line.starts_with("@@") {
                        in_hunk = true;
                    }
                    if line.starts_with("@@") {
                        let mut words = line.split_whitespace();
                        let _ = words.next();
                        old_line = words
                            .next()
                            .and_then(|v| v.trim_start_matches('-').split(',').next()?.parse().ok())
                            .unwrap_or(0);
                        new_line = words
                            .next()
                            .and_then(|v| v.trim_start_matches('+').split(',').next()?.parse().ok())
                            .unwrap_or(0);
                    }
                    let (color, bg) = if line.starts_with('+') && !header {
                        (GREEN, Color32::from_rgb(27, 48, 40))
                    } else if line.starts_with('-') && !header {
                        (
                            Color32::from_rgb(235, 139, 145),
                            Color32::from_rgb(54, 34, 39),
                        )
                    } else if line.starts_with("@@") {
                        (BLUE, Color32::from_rgb(31, 43, 59))
                    } else {
                        (if header { MUTED } else { TEXT }, PANEL)
                    };
                    let (old, new) = if header || line.starts_with('\\') {
                        (String::new(), String::new())
                    } else if line.starts_with('+') {
                        let n = new_line.to_string();
                        new_line += 1;
                        (String::new(), n)
                    } else if line.starts_with('-') {
                        let n = old_line.to_string();
                        old_line += 1;
                        (n, String::new())
                    } else {
                        let o = old_line.to_string();
                        let n = new_line.to_string();
                        old_line += 1;
                        new_line += 1;
                        (o, n)
                    };
                    let galley = if self.wrap_diff {
                        ui.painter().layout(
                            line.to_owned(),
                            FontId::monospace(12.),
                            color,
                            (ui.available_width() - 120.).max(60.),
                        )
                    } else {
                        ui.painter()
                            .layout_no_wrap(line.to_owned(), FontId::monospace(12.), color)
                    };
                    let width = ui.available_width().max(galley.size().x + 120.);
                    let height = (galley.size().y + 4.).max(19.);
                    let (rect, response) = ui.allocate_exact_size(
                        egui::vec2(width, height),
                        if selectable.contains(&i) {
                            egui::Sense::click()
                        } else {
                            egui::Sense::hover()
                        },
                    );
                    if response.hovered() && ui.ctx().input(|input| input.pointer.any_pressed()) {
                        self.focused_history_table = false;
                    }
                    let selected = self.selected_diff_lines.contains(&i) && interactive;
                    ui.painter().rect_filled(
                        rect,
                        0.,
                        if selected {
                            Color32::from_rgb(45, 68, 92)
                        } else {
                            bg
                        },
                    );
                    if selectable.contains(&i) {
                        let mark = egui::Rect::from_min_size(
                            rect.min + egui::vec2(4., 4.),
                            egui::vec2(10., 10.),
                        );
                        ui.painter().rect_stroke(
                            mark,
                            1.,
                            Stroke::new(1., if selected { BLUE } else { MUTED }),
                            egui::StrokeKind::Inside,
                        );
                        if selected {
                            ui.painter().rect_filled(mark.shrink(2.), 0., BLUE);
                        }
                        if response.clicked() && !self.selected_diff_lines.remove(&i) {
                            self.selected_diff_lines.insert(i);
                        }
                    }
                    for (offset, number) in [(49., old), (90., new)] {
                        ui.painter().text(
                            egui::pos2(rect.left() + offset, rect.top() + 9.5),
                            egui::Align2::RIGHT_CENTER,
                            number,
                            FontId::monospace(11.),
                            MUTED,
                        );
                    }
                    ui.painter().line_segment(
                        [
                            rect.left_top() + egui::vec2(100., 0.),
                            rect.left_bottom() + egui::vec2(100., 0.),
                        ],
                        Stroke::new(1., BORDER),
                    );
                    ui.painter()
                        .galley(rect.min + egui::vec2(110., 2.), galley, color);
                }
            });
    }
    pub(super) fn conflict_controls(&mut self, ui: &mut egui::Ui) {
        if let Some(repo) = self.repo() {
            let state = repo.state();
            let operation = match state {
                git2::RepositoryState::Rebase
                | git2::RepositoryState::RebaseInteractive
                | git2::RepositoryState::RebaseMerge => Some(("Rebase", "rebase")),
                git2::RepositoryState::Merge => Some(("Merge", "merge")),
                git2::RepositoryState::CherryPick | git2::RepositoryState::CherryPickSequence => {
                    Some(("Cherry-pick", "cherry-pick"))
                }
                git2::RepositoryState::Revert | git2::RepositoryState::RevertSequence => {
                    Some(("Revert", "revert"))
                }
                _ => None,
            };
            if let Some((name, command)) = operation {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(format!("{name} in progress")).color(BLUE));
                    let conflicts = repo.index().is_ok_and(|i| i.has_conflicts());
                    if ui
                        .add_enabled(!conflicts, egui::Button::new(format!("Continue {name}")))
                        .clicked()
                    {
                        if command == "merge" {
                            self.git_action("Complete merge", &["commit", "--no-edit"]);
                        } else {
                            self.git_action(
                                &format!("Continue {name}"),
                                &["-c", "core.editor=true", command, "--continue"],
                            );
                        }
                    }
                    if ui.button(format!("Abort {name}")).clicked() {
                        self.git_action(&format!("Abort {name}"), &[command, "--abort"]);
                    }
                });
                ui.separator();
            }
        }
        if let Some(repo) = self.repo() {
            let is_rebase = matches!(
                repo.state(),
                git2::RepositoryState::Rebase
                    | git2::RepositoryState::RebaseInteractive
                    | git2::RepositoryState::RebaseMerge
            );
            let mut conflicts = Vec::new();
            if let Ok(index) = repo.index()
                && let Ok(entries) = index.conflicts()
            {
                for conflict in entries.flatten() {
                    if let Some(path) = conflict
                        .our
                        .or(conflict.their)
                        .or(conflict.ancestor)
                        .and_then(|entry| String::from_utf8(entry.path).ok())
                    {
                        conflicts.push(path);
                    }
                }
            }
            if !conflicts.is_empty() {
                egui::Frame::new()
                    .fill(Color32::from_rgb(57, 40, 34))
                    .stroke(Stroke::new(1., Color32::from_rgb(145, 94, 58)))
                    .corner_radius(8)
                    .inner_margin(8.)
                    .show(ui, |ui| {
                        ui.label(
                            RichText::new(format!(
                                "{} conflicts · {}",
                                if is_rebase { "Rebase" } else { "Merge" },
                                conflicts.len()
                            ))
                                .strong()
                                .color(Color32::from_rgb(255, 195, 128)),
                        );
                        for path in conflicts {
                            ui.horizontal(|ui| {
                                if ui
                                    .selectable_label(
                                        self.selected_file.as_deref() == Some(&path),
                                        RichText::new(&path).color(TEXT),
                                    )
                                    .clicked()
                                {
                                    self.select_file(path.clone());
                                    self.edit_conflict(&path);
                                }
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    if ui
                                        .add_enabled(
                                            self.operation.is_none(),
                                            egui::Button::new("Mark resolved"),
                                        )
                                        .clicked()
                                    {
                                        self.resolve_conflict(&path, None);
                                    }
                                    if ui
                                        .add_enabled(
                                            self.operation.is_none(),
                                            egui::Button::new(if is_rebase {
                                                "Replayed commit"
                                            } else {
                                                "Their version"
                                            }),
                                        )
                                        .on_hover_text(if is_rebase {
                                            "Use the version from the commit being replayed (--theirs)"
                                        } else {
                                            "Use the version from the other branch (--theirs)"
                                        })
                                        .clicked()
                                    {
                                        self.resolve_conflict(&path, Some("--theirs"));
                                    }
                                    if ui
                                        .add_enabled(
                                            self.operation.is_none(),
                                            egui::Button::new(if is_rebase {
                                                "Upstream version"
                                            } else {
                                                "Our version"
                                            }),
                                        )
                                        .on_hover_text(if is_rebase {
                                            "Use the version from the branch being rebased onto (--ours)"
                                        } else {
                                            "Use the version from the current branch (--ours)"
                                        })
                                        .clicked()
                                    {
                                        self.resolve_conflict(&path, Some("--ours"));
                                    }
                                });
                            });
                        }
                    });
                ui.add_space(6.);
            }
        }
        let mut save_conflict = None;
        let mut cancel_conflict_edit = false;
        if let Some((path, content)) = &mut self.conflict_draft {
            egui::Frame::new()
                .fill(PANEL)
                .stroke(Stroke::new(1., BORDER))
                .corner_radius(8)
                .inner_margin(8.)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(format!("Resolve · {path}"))
                                .strong()
                                .color(TEXT),
                        );
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui
                                .add_enabled(
                                    self.operation.is_none(),
                                    egui::Button::new("Save and mark resolved"),
                                )
                                .clicked()
                            {
                                save_conflict = Some((path.clone(), content.clone()));
                            }
                            if ui.button("Cancel edit").clicked() {
                                cancel_conflict_edit = true;
                            }
                        });
                    });
                    ui.add(
                        egui::TextEdit::multiline(content)
                            .code_editor()
                            .desired_rows(12)
                            .desired_width(f32::INFINITY),
                    );
                });
        }
        if cancel_conflict_edit {
            self.conflict_draft = None;
        }
        if let Some((path, content)) = save_conflict
            && let Some(root) = &self.path
        {
            match std::fs::write(root.join(&path), content) {
                Ok(()) => {
                    self.conflict_draft = None;
                    self.resolve_conflict(&path, None);
                }
                Err(error) => self.notice = format!("Could not save conflict file: {error}"),
            }
        }
    }
    pub(super) fn patch_editor(&mut self, ui: &mut egui::Ui) {
        let mut apply_patch = None;
        let mut cancel_patch = false;
        if let Some((path, patch)) = &mut self.patch_draft {
            egui::Frame::new()
                .fill(PANEL)
                .stroke(Stroke::new(1., BORDER))
                .corner_radius(8)
                .inner_margin(8.)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(format!("Partial staging · {path}"))
                                .strong()
                                .color(TEXT),
                        );
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui
                                .add_enabled(
                                    self.operation.is_none() && patch_has_changes(patch),
                                    egui::Button::new("Stage edited patch"),
                                )
                                .clicked()
                            {
                                apply_patch = Some(patch.clone());
                            }
                            if ui.button("Cancel").clicked() {
                                cancel_patch = true;
                            }
                        });
                    });
                    ui.label(
                        RichText::new("Keep + or − lines you want staged. Remove unwanted + lines. To leave a − line unstaged, change its prefix to a space so it remains as context.")
                            .small()
                            .color(MUTED),
                    );
                    ui.add(
                        egui::TextEdit::multiline(patch)
                            .code_editor()
                            .desired_rows(12)
                            .desired_width(f32::INFINITY),
                    );
                });
        }
        if cancel_patch {
            self.patch_draft = None;
        }
        if let Some(patch) = apply_patch {
            self.apply_stage_patch(&patch);
        }
    }
}
