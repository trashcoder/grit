use std::collections::BTreeMap;
use std::collections::HashMap;
use std::hash::Hash;

use eframe::egui;

/// An action emitted by the file tree. Paths are always the original input paths.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum TreeAction {
    Select(String),
    FileHistory(String),
    CopyPath(String),
}

#[derive(Debug, PartialEq, Eq)]
struct TreeNode {
    name: String,
    path: String,
    file: Option<String>,
    children: BTreeMap<String, TreeNode>,
}

impl TreeNode {
    fn new(name: String, path: String) -> Self {
        Self {
            name,
            path,
            file: None,
            children: BTreeMap::new(),
        }
    }
}

fn build_tree(paths: &[String]) -> BTreeMap<String, TreeNode> {
    let mut roots = BTreeMap::new();
    for original in paths {
        // Git paths use '/' as their separator on every platform. A backslash is
        // an ordinary filename character and must never be treated as a separator.
        let parts: Vec<&str> = original
            .split('/')
            .filter(|part| !part.is_empty())
            .collect();
        if parts.is_empty() {
            continue;
        }

        let mut siblings = &mut roots;
        let mut parent_path = String::new();
        for (index, part) in parts.iter().enumerate() {
            let path = if parent_path.is_empty() {
                (*part).to_owned()
            } else {
                format!("{parent_path}/{part}")
            };
            let node = siblings
                .entry((*part).to_owned())
                .or_insert_with(|| TreeNode::new((*part).to_owned(), path.clone()));
            if index + 1 == parts.len() {
                node.file = Some(original.clone());
            }
            parent_path = path;
            siblings = &mut node.children;
        }
    }
    roots
}

/// Draw a nested file tree. Each disclosure state is keyed by namespace and the
/// complete displayed path, so a path in two lists has independent expansion.
pub(super) fn file_tree_ui(
    ui: &mut egui::Ui,
    namespace: impl Hash,
    paths: &[String],
    selected: Option<&str>,
) -> Option<TreeAction> {
    file_tree_impl(ui, namespace, paths, selected, None)
}

/// Draw a tree with a compact, colored status marker before each leaf name.
pub(super) fn file_tree_status_ui(
    ui: &mut egui::Ui,
    namespace: impl Hash,
    paths: &[String],
    selected: Option<&str>,
    statuses: &HashMap<String, &'static str>,
) -> Option<TreeAction> {
    file_tree_impl(ui, namespace, paths, selected, Some(statuses))
}

fn file_tree_impl(
    ui: &mut egui::Ui,
    namespace: impl Hash,
    paths: &[String],
    selected: Option<&str>,
    statuses: Option<&HashMap<String, &'static str>>,
) -> Option<TreeAction> {
    let tree = build_tree(paths);
    let mut action = None;
    for node in sorted_nodes(&tree) {
        show_node(ui, &namespace, node, selected, statuses, &mut action);
    }
    action
}

fn show_node(
    ui: &mut egui::Ui,
    namespace: &impl Hash,
    node: &TreeNode,
    selected: Option<&str>,
    statuses: Option<&HashMap<String, &'static str>>,
    action: &mut Option<TreeAction>,
) {
    if !node.children.is_empty() {
        let id = ui.make_persistent_id(("folder", namespace, &node.path));
        let mut clicked = false;
        let mut header =
            egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, false)
                .show_header(ui, |ui| {
                    let width = ui.available_width().max(0.);
                    let response = ui.add_sized(
                        [width, ui.spacing().interact_size.y],
                        egui::Label::new(egui::RichText::new(&node.name).strong())
                            .truncate()
                            .sense(egui::Sense::click()),
                    );
                    clicked = response.clicked();
                    response
                });
        if clicked {
            header.toggle();
        }
        let (_, header_response, _) = header.body(|ui| {
            for child in sorted_nodes(&node.children) {
                show_node(ui, namespace, child, selected, statuses, action);
            }
        });
        header_response.response.on_hover_text(&node.path);
        // A node can represent both a file and a directory (for example `a`
        // alongside `a/b`). Keep the file available as a sibling row.
        if let Some(path) = &node.file {
            show_file(
                ui,
                namespace,
                &node.name,
                path,
                selected,
                statuses.and_then(|map| map.get(path).copied()),
                action,
            );
        }
    } else if let Some(path) = &node.file {
        show_file(
            ui,
            namespace,
            &node.name,
            path,
            selected,
            statuses.and_then(|map| map.get(path).copied()),
            action,
        );
    }
}

fn sorted_nodes(nodes: &BTreeMap<String, TreeNode>) -> Vec<&TreeNode> {
    let mut sorted: Vec<_> = nodes.values().collect();
    sorted.sort_by(|a, b| {
        a.children
            .is_empty()
            .cmp(&b.children.is_empty())
            .then_with(|| a.name.cmp(&b.name))
    });
    sorted
}

fn show_file(
    ui: &mut egui::Ui,
    namespace: &impl Hash,
    name: &str,
    path: &str,
    selected: Option<&str>,
    status: Option<&'static str>,
    action: &mut Option<TreeAction>,
) {
    let width = ui.available_width().max(0.);
    let response = ui
        .push_id(("file", namespace, path), |ui| {
            ui.horizontal(|ui| {
                if let Some(status) = status {
                    let color = match status {
                        "A" | "??" => egui::Color32::from_rgb(116, 190, 132),
                        "D" => egui::Color32::from_rgb(220, 112, 112),
                        _ => egui::Color32::from_rgb(230, 184, 92),
                    };
                    ui.add_sized(
                        [24., 24.],
                        egui::Label::new(egui::RichText::new(status).monospace().color(color)),
                    );
                }
                let name_width = (width - if status.is_some() { 30. } else { 0. }).max(0.);
                ui.add_sized(
                    [name_width, 24.],
                    egui::Button::new(truncate_name(name, 48))
                        .selected(selected == Some(path))
                        .frame(false)
                        .truncate(),
                )
            })
            .inner
        })
        .inner;
    if response.clicked() {
        *action = Some(TreeAction::Select(path.to_owned()));
    }
    let response = response.on_hover_text(path);
    response.context_menu(|ui| {
        if ui.button("File history…").clicked() {
            *action = Some(TreeAction::FileHistory(path.to_owned()));
            ui.close_menu();
        }
        if ui.button("Copy path").clicked() {
            *action = Some(TreeAction::CopyPath(path.to_owned()));
            ui.close_menu();
        }
    });
}

fn truncate_name(name: &str, max_chars: usize) -> String {
    let mut chars = name.chars();
    let prefix: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{prefix}…")
    } else {
        prefix
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leaf_paths(nodes: &BTreeMap<String, TreeNode>, output: &mut Vec<String>) {
        for node in nodes.values() {
            if let Some(path) = &node.file {
                output.push(path.clone());
            }
            leaf_paths(&node.children, output);
        }
    }

    #[test]
    fn builds_nested_tree_and_preserves_exact_leaf_paths() {
        let paths = vec![
            "src/main.rs".into(),
            "README.md".into(),
            "src/lib.rs".into(),
            "src/deep/λ.txt".into(),
        ];
        let tree = build_tree(&paths);
        let mut leaves = Vec::new();
        leaf_paths(&tree, &mut leaves);
        leaves.sort();
        let mut expected = paths;
        expected.sort();
        assert_eq!(leaves, expected);
        assert_eq!(
            tree.keys().cloned().collect::<Vec<_>>(),
            ["README.md", "src"]
        );
        assert_eq!(
            tree["src"].children.keys().cloned().collect::<Vec<_>>(),
            ["deep", "lib.rs", "main.rs"]
        );
    }

    #[test]
    fn file_and_directory_can_share_a_prefix() {
        let tree = build_tree(&["a".into(), "a/b".into()]);
        assert_eq!(tree["a"].file.as_deref(), Some("a"));
        assert_eq!(tree["a"].children["b"].file.as_deref(), Some("a/b"));
    }

    #[test]
    fn ordering_does_not_depend_on_insertion_order() {
        let a = build_tree(&["z/f".into(), "a/g".into(), "z/e".into()]);
        let b = build_tree(&["z/e".into(), "z/f".into(), "a/g".into()]);
        assert_eq!(a, b);
    }

    #[test]
    fn backslash_is_literal_and_root_file_is_preserved() {
        let paths = vec![r"folder\file.txt".into(), "root.txt".into()];
        let tree = build_tree(&paths);
        assert!(tree.contains_key(r"folder\file.txt"));
        assert!(tree["root.txt"].children.is_empty());
        assert_eq!(
            tree[r"folder\file.txt"].file.as_deref(),
            Some(r"folder\file.txt")
        );
    }
}
