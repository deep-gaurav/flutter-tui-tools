use crate::vm_service::RemoteDiagnosticsNode;

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
}

impl AppState {
    pub fn new() -> Self {
        Self {
            root_node: None,
            connection_status: "Connecting...".to_string(),
            selected_index: 0,
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
}
