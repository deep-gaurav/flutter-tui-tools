use crate::vm_service::RemoteDiagnosticsNode;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Focus {
    Tree,
    Details,
    Logs,
}

pub struct AppState {
    pub root_node: Option<RemoteDiagnosticsNode>,
    pub connection_status: String,
    // Flattened view for rendering and navigation: (depth, node)
    // We store indices or references if possible, but self-referential structs are hard in Rust.
    // So we'll rebuild this on demand or store IDs/indices.
    // For simplicity, let's just store the visible nodes as a list of references?
    // No, lifetimes issues.
    // Let's store a list of (depth, &RemoteDiagnosticsNode) is tricky with lifetimes.
    // Let's just store the list of visible node IDs and their depth?
    // Or just rebuild the flat list every draw?
    // Rebuilding every draw is fine for TUI frame rates.
    pub selected_index: usize,

    pub logs: Vec<String>,
    pub log_scroll_state: usize, // Index of the first visible log line (or offset)
    pub log_auto_scroll: bool,

    pub focus: Focus,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            root_node: None,
            connection_status: "Connecting...".to_string(),
            selected_index: 0,
            logs: Vec::new(),
            log_scroll_state: 0,
            log_auto_scroll: true,
            focus: Focus::Tree,
        }
    }

    pub fn set_root_node(&mut self, node: RemoteDiagnosticsNode) {
        self.root_node = Some(node);
        // Reset selection on new tree
        self.selected_index = 0;
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
        // If we supported expansion, we'd check expanded_node_ids here.
        // For now, everything is expanded.
        if let Some(children) = &node.children {
            for child in children {
                self.count_visible(child, count);
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
    }

    pub fn add_log(&mut self, message: String) {
        self.logs.push(message);
        if self.log_auto_scroll {
            // We'll calculate the correct offset during rendering or just set it to a large number
            // and let the UI clamp it. Or simpler: just track if we are at the bottom.
            // For now, let's just say if auto-scroll is on, we don't change scroll_state manually here,
            // but the UI will use logs.len() - height.
            // Actually, let's just store the index of the *first* visible line.
            // If auto-scroll is on, we'll update it in the UI draw or here if we knew the height.
            // Since we don't know height here, let's just use a flag.
        }
    }

    pub fn scroll_logs(&mut self, delta: isize) {
        if delta < 0 {
            self.log_auto_scroll = false;
            let new_scroll = self.log_scroll_state as isize + delta;
            self.log_scroll_state = new_scroll.max(0) as usize;
        } else {
            let new_scroll = self.log_scroll_state as isize + delta;
            // We can't clamp to max without knowing height, but we can clamp to logs.len()
            self.log_scroll_state = (new_scroll as usize).min(self.logs.len().saturating_sub(1));

            // Re-enable auto-scroll if we hit the bottom
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
