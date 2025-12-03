use crate::vm_service::RemoteDiagnosticsNode;
use std::collections::HashSet;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Focus {
    Tree,
    Details,
    Logs,
    IsolateSelection,
    Search,
}

pub struct AppState {
    pub root_node: Option<RemoteDiagnosticsNode>,
    pub selected_node_details: Option<RemoteDiagnosticsNode>,
    pub connection_status: String,

    // Isolate Selection State
    pub available_isolates: Vec<crate::vm_service::IsolateRef>,
    pub show_isolate_selection: bool,
    pub selected_isolate_index: usize,

    // Tree State
    pub selected_index: usize,
    pub expanded_ids: HashSet<String>,
    pub tree_scroll_offset: usize,
    pub tree_horizontal_scroll: usize,

    // Logs State
    pub logs: Vec<String>,
    pub log_scroll_state: usize, // Index of the first visible log line
    pub log_auto_scroll: bool,

    // Search State
    pub search_query: String,
    pub search_results: Vec<String>, // IDs of matching nodes
    pub current_match_index: usize,  // Index into search_results

    pub focus: Focus,

    pub tx_flutter_command: Option<tokio::sync::mpsc::Sender<String>>,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            root_node: None,
            selected_node_details: None,
            connection_status: "Connecting...".to_string(),
            available_isolates: Vec::new(),
            show_isolate_selection: false,
            selected_isolate_index: 0,
            selected_index: 0,
            expanded_ids: HashSet::new(),
            tree_scroll_offset: 0,
            tree_horizontal_scroll: 0,
            logs: Vec::new(),
            log_scroll_state: 0,
            log_auto_scroll: true,
            search_query: String::new(),
            search_results: Vec::new(),
            current_match_index: 0,
            focus: Focus::Tree,
            tx_flutter_command: None,
        }
    }

    pub fn set_root_node(&mut self, node: RemoteDiagnosticsNode) {
        // Capture currently selected node ID
        let selected_id = self.get_selected_node().and_then(|n| Self::get_node_id(n));

        // When we get a new tree, we might want to preserve expansion state if possible.
        // For now, let's just expand the root by default.
        if let Some(id) = Self::get_node_id(&node) {
            self.expanded_ids.insert(id);
        }
        self.root_node = Some(node);

        // Try to restore selection
        if let Some(id) = selected_id {
            // Ensure path is expanded (in case IDs changed or it's a new tree structure)
            self.expand_path_to_node(&id);

            if let Some(index) = self.get_visible_index_of_id(&id) {
                self.selected_index = index;
                // Update scroll to keep it visible
                self.ensure_selection_visible_after_restore();
            } else {
                // Node not found, reset to top
                self.selected_index = 0;
                self.tree_scroll_offset = 0;
                self.selected_node_details = None;
            }
        } else {
            // No previous selection, reset
            self.selected_index = 0;
            self.tree_scroll_offset = 0;
            self.selected_node_details = None;
        }
    }

    fn ensure_selection_visible_after_restore(&mut self) {
        if self.selected_index < self.tree_scroll_offset {
            self.tree_scroll_offset = self.selected_index;
        } else {
            // We don't know the height here, so we can't perfectly scroll to bottom.
            // But we can try to keep it somewhat centered or just ensure top visibility.
            // Let's just leave scroll offset alone if it's visible, or move it if it's way off.
            // Actually, if we just reloaded, the tree structure might be similar.
            // Let's try to maintain relative position?
            // For now, let's just ensure it's not above the viewport.
            // If it's way below, the user will scroll.
            // Better: use the same logic as jump_to_match
            if self.selected_index >= 3 {
                self.tree_scroll_offset = self.selected_index - 3;
            } else {
                self.tree_scroll_offset = 0;
            }
        }
    }

    pub fn get_node_id(node: &RemoteDiagnosticsNode) -> Option<String> {
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
    pub fn get_selected_node(&self) -> Option<&RemoteDiagnosticsNode> {
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
                self.selected_node_details = None;
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
        self.selected_node_details = None;
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

    pub fn scroll_tree_horizontal(&mut self, delta: isize) {
        let new_offset = self.tree_horizontal_scroll as isize + delta;
        self.tree_horizontal_scroll = new_offset.max(0) as usize;
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

    pub fn get_selected_depth(&self) -> usize {
        if let Some(root) = &self.root_node {
            let mut current_index = 0;
            return self
                .find_depth_at_index(root, &mut current_index, 0)
                .unwrap_or(0);
        }
        0
    }

    fn find_depth_at_index(
        &self,
        node: &RemoteDiagnosticsNode,
        current_index: &mut usize,
        depth: usize,
    ) -> Option<usize> {
        if *current_index == self.selected_index {
            return Some(depth);
        }
        *current_index += 1;

        if let Some(id) = Self::get_node_id(node) {
            if self.expanded_ids.contains(&id) {
                if let Some(children) = &node.children {
                    for child in children {
                        if let Some(found) =
                            self.find_depth_at_index(child, current_index, depth + 1)
                        {
                            return Some(found);
                        }
                    }
                }
            }
        }
        None
    }

    pub fn ensure_horizontal_visibility(&mut self, viewport_width: usize) {
        let depth = self.get_selected_depth();
        let start_visual_pos = depth * 2; // Assuming 2 spaces per indent
        let padding = 2;

        if start_visual_pos < self.tree_horizontal_scroll + padding {
            self.tree_horizontal_scroll = start_visual_pos.saturating_sub(padding);
        } else if start_visual_pos
            > self.tree_horizontal_scroll + viewport_width.saturating_sub(padding)
        {
            // Scroll right to make it visible, but not too far.
            // We want start_visual_pos to be visible.
            // Let's scroll so start_visual_pos is at 1/3 of the screen or just visible?
            // User said: "not too far right".
            // Let's try to put it at the left edge + padding.
            // Wait, if we scroll right, we increase tree_horizontal_scroll.
            // If start_visual_pos is 100, and scroll is 0, width is 50. 100 > 50.
            // We want scroll to be such that 100 is visible.
            // If we set scroll = 100 - width + padding, then 100 is at right edge.
            // If we set scroll = 100 - padding, then 100 is at left edge.
            // User wants "intelligently scroll left or right".
            // "scroll left when user scrolls up... and widget is out of screen" -> implies bringing it into view from left.
            // "scroll right if user moves to a widget which is tooo far to right" -> implies bringing it into view from right.

            // Let's aim to keep it within the viewport.
            // If it's off to the right, bring it to the right edge with some padding?
            // Or maybe center it? No, centering might be too jumpy.
            // Let's just ensure it's visible.

            self.tree_horizontal_scroll = start_visual_pos + padding + 10 - viewport_width;
        }
    }

    pub fn cycle_focus(&mut self) {
        if self.show_isolate_selection {
            return; // Lock focus when selecting isolate
        }
        self.focus = match self.focus {
            Focus::Tree => Focus::Details,
            Focus::Details => Focus::Logs,
            Focus::Logs => Focus::Tree,
            Focus::Search => Focus::Tree, // Cycle back to tree from search
            Focus::IsolateSelection => Focus::IsolateSelection, // Should not happen if locked
        };
    }

    pub fn move_isolate_selection(&mut self, delta: isize) {
        if self.available_isolates.is_empty() {
            return;
        }
        let new_index = self.selected_isolate_index as isize + delta;
        if new_index < 0 {
            self.selected_isolate_index = 0;
        } else if new_index >= self.available_isolates.len() as isize {
            self.selected_isolate_index = self.available_isolates.len() - 1;
        } else {
            self.selected_isolate_index = new_index as usize;
        }
    }

    pub fn focus_selected_node(&mut self) {
        // Position the selected node at the top-left of the viewport
        self.tree_scroll_offset = self.selected_index;

        let depth = self.get_selected_depth();
        let start_visual_pos = depth * 2; // Assuming 2 spaces per indent
        self.tree_horizontal_scroll = start_visual_pos;
    }

    pub fn perform_search(&mut self) {
        self.search_results.clear();
        self.current_match_index = 0;

        if self.search_query.is_empty() {
            return;
        }

        use fuzzy_matcher::skim::SkimMatcherV2;
        let matcher = SkimMatcherV2::default();

        let mut results = Vec::new();
        if let Some(root) = &self.root_node {
            Self::search_recursive(root, &matcher, &self.search_query, &mut results);
        }
        self.search_results = results;

        // Auto-focus first match
        if !self.search_results.is_empty() {
            self.jump_to_match(0);
        }
    }

    fn search_recursive(
        node: &RemoteDiagnosticsNode,
        matcher: &fuzzy_matcher::skim::SkimMatcherV2,
        query: &str,
        results: &mut Vec<String>,
    ) {
        use fuzzy_matcher::FuzzyMatcher;

        let mut match_found = false;
        if let Some(desc) = &node.description {
            if matcher.fuzzy_match(desc, query).is_some() {
                match_found = true;
            }
        }
        if !match_found {
            if let Some(w_type) = &node.widget_runtime_type {
                if matcher.fuzzy_match(w_type, query).is_some() {
                    match_found = true;
                }
            }
        }

        if match_found {
            if let Some(id) = Self::get_node_id(node) {
                results.push(id);
            }
        }

        if let Some(children) = &node.children {
            for child in children {
                Self::search_recursive(child, matcher, query, results);
            }
        }
    }

    pub fn next_match(&mut self) {
        if self.search_results.is_empty() {
            return;
        }
        self.current_match_index = (self.current_match_index + 1) % self.search_results.len();
        self.jump_to_match(self.current_match_index);
    }

    pub fn prev_match(&mut self) {
        if self.search_results.is_empty() {
            return;
        }
        if self.current_match_index == 0 {
            self.current_match_index = self.search_results.len() - 1;
        } else {
            self.current_match_index -= 1;
        }
        self.jump_to_match(self.current_match_index);
    }

    fn jump_to_match(&mut self, match_index: usize) {
        if let Some(id) = self.search_results.get(match_index).cloned() {
            // 1. Expand path to this node
            self.expand_path_to_node(&id);

            // 2. Find the new visible index of this node
            if let Some(index) = self.get_visible_index_of_id(&id) {
                self.selected_index = index;

                // 3. Scroll to show context
                if index >= 3 {
                    self.tree_scroll_offset = index - 3;
                } else {
                    self.tree_scroll_offset = 0;
                }

                let depth = self.get_selected_depth();
                let start_visual_pos = depth * 2;
                self.tree_horizontal_scroll = start_visual_pos.saturating_sub(6);
            }
        }
    }

    fn expand_path_to_node(&mut self, target_id: &str) {
        if let Some(root) = &self.root_node {
            let mut path = Vec::new();
            if Self::find_path_to_node(root, target_id, &mut path) {
                for id in path {
                    self.expanded_ids.insert(id);
                }
            }
        }
    }

    fn find_path_to_node(
        node: &RemoteDiagnosticsNode,
        target_id: &str,
        path: &mut Vec<String>,
    ) -> bool {
        if let Some(id) = Self::get_node_id(node) {
            if id == target_id {
                // Don't necessarily need to add the node itself to expanded_ids,
                // but adding it doesn't hurt (it just expands the node itself).
                // Usually we want to expand parents.
                // But let's add it to path so we can expand it if it has children?
                // Actually, we usually want to see the node, so parents must be expanded.
                // The node itself being expanded is optional.
                // Let's add it.
                // path.push(id); // Optional
                return true;
            }

            path.push(id.clone());
            if let Some(children) = &node.children {
                for child in children {
                    if Self::find_path_to_node(child, target_id, path) {
                        return true;
                    }
                }
            }
            path.pop();
        }
        false
    }

    fn get_visible_index_of_id(&self, target_id: &str) -> Option<usize> {
        if let Some(root) = &self.root_node {
            let mut current_index = 0;
            return self.find_visible_index_recursive(root, target_id, &mut current_index);
        }
        None
    }

    fn find_visible_index_recursive(
        &self,
        node: &RemoteDiagnosticsNode,
        target_id: &str,
        current_index: &mut usize,
    ) -> Option<usize> {
        if let Some(id) = Self::get_node_id(node) {
            if id == target_id {
                return Some(*current_index);
            }

            *current_index += 1;

            if self.expanded_ids.contains(&id) {
                if let Some(children) = &node.children {
                    for child in children {
                        if let Some(found) =
                            self.find_visible_index_recursive(child, target_id, current_index)
                        {
                            return Some(found);
                        }
                    }
                }
            }
        }
        None
    }
}
