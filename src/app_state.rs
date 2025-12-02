use crate::vm_service::RemoteDiagnosticsNode;
use std::collections::HashSet;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Focus {
    Tree,
    Details,
    Logs,
}

pub struct AppState {
    pub root_node: Option<RemoteDiagnosticsNode>,
    pub connection_status: String,

    // Tree State
    pub selected_index: usize,
    pub expanded_ids: HashSet<String>,
    pub tree_scroll_offset: usize,
    pub tree_horizontal_scroll: usize,

    // Logs State
    pub logs: Vec<String>,
    pub log_scroll_state: usize, // Index of the first visible log line
    pub log_auto_scroll: bool,

    pub focus: Focus,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            root_node: None,
            connection_status: "Connecting...".to_string(),
            selected_index: 0,
            expanded_ids: HashSet::new(),
            tree_scroll_offset: 0,
            tree_horizontal_scroll: 0,
            logs: Vec::new(),
            log_scroll_state: 0,
            log_auto_scroll: true,
            focus: Focus::Tree,
        }
    }

    pub fn set_root_node(&mut self, node: RemoteDiagnosticsNode) {
        // When we get a new tree, we might want to preserve expansion state if possible.
        // For now, let's just expand the root by default.
        if let Some(id) = Self::get_node_id(&node) {
            self.expanded_ids.insert(id);
        }
        self.root_node = Some(node);
        // Reset selection on new tree
        self.selected_index = 0;
        self.tree_scroll_offset = 0;
    }

    fn get_node_id(node: &RemoteDiagnosticsNode) -> Option<String> {
        // Prefer value_id, then object_id, then maybe something else?
        // value_id seems to be the persistent ID for the widget in the inspector.
        node.value_id.clone().or(node.object_id.clone())
    }

    pub fn toggle_expand(&mut self) {
        if let Some(node) = self.get_selected_node() {
            if let Some(id) = Self::get_node_id(node) {
                if self.expanded_ids.contains(&id) {
                    self.expanded_ids.remove(&id);
                } else {
                    self.expanded_ids.insert(id);
                }
            }
        }
    }

    pub fn expand_selected(&mut self) {
        if let Some(node) = self.get_selected_node() {
            // We need to clone the node or IDs to avoid borrowing issues while mutating self.expanded_ids
            // But we can't clone RemoteDiagnosticsNode easily if it's large, but it's just data.
            // Actually, we just need to collect IDs to expand.
            let mut ids_to_expand = Vec::new();
            Self::collect_smart_expand_ids(node, &mut ids_to_expand, 5);

            for id in ids_to_expand {
                self.expanded_ids.insert(id);
            }
        }
    }

    fn collect_smart_expand_ids(
        node: &RemoteDiagnosticsNode,
        ids: &mut Vec<String>,
        depth_limit: usize,
    ) {
        if let Some(id) = Self::get_node_id(node) {
            ids.push(id);

            if depth_limit > 0 {
                if let Some(children) = &node.children {
                    if children.len() == 1 {
                        Self::collect_smart_expand_ids(&children[0], ids, depth_limit - 1);
                    }
                }
            }
        }
    }

    pub fn collapse_selected(&mut self) -> bool {
        if let Some(node) = self.get_selected_node() {
            if let Some(id) = Self::get_node_id(node) {
                if self.expanded_ids.contains(&id) {
                    self.expanded_ids.remove(&id);
                    return true;
                }
            }
        }
        false
    }

    // Helper to find the node at the current selected index based on visible nodes
    fn get_selected_node(&self) -> Option<&RemoteDiagnosticsNode> {
        if let Some(root) = &self.root_node {
            let mut current_index = 0;
            return self.find_node_at_index(root, &mut current_index);
        }
        None
    }

    fn find_node_at_index<'a>(
        &'a self,
        node: &'a RemoteDiagnosticsNode,
        current_index: &mut usize,
    ) -> Option<&'a RemoteDiagnosticsNode> {
        if *current_index == self.selected_index {
            return Some(node);
        }
        *current_index += 1;

        if let Some(id) = Self::get_node_id(node) {
            if self.expanded_ids.contains(&id) {
                if let Some(children) = &node.children {
                    for child in children {
                        if let Some(found) = self.find_node_at_index(child, current_index) {
                            return Some(found);
                        }
                    }
                }
            }
        }
        None
    }

    // Helper to get parent of currently selected node (for Left arrow navigation)
    // This is expensive to traverse every time, but tree size is likely manageable for now.
    pub fn select_parent(&mut self) {
        if let Some(root) = &self.root_node {
            let mut current_index = 0;
            if let Some(parent_index) = self.find_parent_index(root, &mut current_index, None) {
                self.selected_index = parent_index;
                self.ensure_selection_visible();
            }
        }
    }

    fn find_parent_index(
        &self,
        node: &RemoteDiagnosticsNode,
        current_index: &mut usize,
        parent_index: Option<usize>,
    ) -> Option<usize> {
        if *current_index == self.selected_index {
            return parent_index;
        }

        let my_index = *current_index;
        *current_index += 1;

        if let Some(id) = Self::get_node_id(node) {
            if self.expanded_ids.contains(&id) {
                if let Some(children) = &node.children {
                    for child in children {
                        if let Some(found) =
                            self.find_parent_index(child, current_index, Some(my_index))
                        {
                            return Some(found);
                        }
                    }
                }
            }
        }
        None
    }

    pub fn visible_count(&self) -> usize {
        if let Some(root) = &self.root_node {
            let mut count = 0;
            self.count_visible(root, &mut count);
            count
        } else {
            0
        }
    }

    fn count_visible(&self, node: &RemoteDiagnosticsNode, count: &mut usize) {
        *count += 1;
        if let Some(id) = Self::get_node_id(node) {
            if self.expanded_ids.contains(&id) {
                if let Some(children) = &node.children {
                    for child in children {
                        self.count_visible(child, count);
                    }
                }
            }
        }
    }

    pub fn move_selection(&mut self, delta: isize) {
        let count = self.visible_count();
        if count == 0 {
            return;
        }

        let new_index = self.selected_index as isize + delta;
        if new_index < 0 {
            self.selected_index = 0;
        } else if new_index >= count as isize {
            self.selected_index = count - 1;
        } else {
            self.selected_index = new_index as usize;
        }
        self.ensure_selection_visible();
    }

    pub fn ensure_selection_visible(&mut self) {
        // We need to know the height of the viewport to do this correctly,
        // but we don't have it here.
        // We'll handle the "scroll into view" logic in the UI draw or
        // pass the height here.
        // For now, let's just assume a safe default or handle it in the draw loop?
        // Actually, standard practice is to update scroll_offset here if we can.
        // But we don't know the viewport height.
        // Let's add a method `update_scroll_for_viewport` that the UI calls.
    }

    pub fn update_tree_scroll(&mut self, height: usize) {
        if self.selected_index < self.tree_scroll_offset {
            self.tree_scroll_offset = self.selected_index;
        } else if self.selected_index >= self.tree_scroll_offset + height {
            self.tree_scroll_offset = self.selected_index - height + 1;
        }
    }

    pub fn scroll_tree(&mut self, delta: isize) {
        let new_offset = self.tree_scroll_offset as isize + delta;
        self.tree_scroll_offset = new_offset.max(0) as usize;
        // We can't cap it easily without knowing total count, but that's fine,
        // rendering will handle empty space.
    }

    pub fn add_log(&mut self, message: String) {
        self.logs.push(message);
        // If auto-scroll is on, we don't strictly need to do anything here
        // if the UI handles "tailing".
    }

    pub fn scroll_logs(&mut self, delta: isize) {
        if delta < 0 {
            self.log_auto_scroll = false;
            let new_scroll = self.log_scroll_state as isize + delta;
            self.log_scroll_state = new_scroll.max(0) as usize;
        } else {
            let new_scroll = self.log_scroll_state as isize + delta;
            self.log_scroll_state = (new_scroll as usize).min(self.logs.len().saturating_sub(1));

            if self.log_scroll_state >= self.logs.len().saturating_sub(1) {
                self.log_auto_scroll = true;
            }
        }
    }

    pub fn cycle_focus(&mut self) {
        self.focus = match self.focus {
            Focus::Tree => Focus::Details,
            Focus::Details => Focus::Logs,
            Focus::Logs => Focus::Tree,
        };
    }
}
