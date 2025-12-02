use crate::app_state::AppState;
use ratatui::{
    layout::Rect,
    widgets::{Block, Borders, Paragraph},
    Frame,
};

pub fn draw(f: &mut Frame, area: Rect, state: &AppState) {
    let block = Block::default().title("Details").borders(Borders::ALL);

    let content = if let Some(root) = &state.root_node {
        // Re-flatten to find the selected node by index
        // TODO: Optimize this by storing the flattened list or using a better data structure
        let mut lines = Vec::new();
        let mut visible_nodes = Vec::new();
        flatten_tree(root, 0, &mut lines, &mut visible_nodes);

        if let Some(node) = visible_nodes.get(state.selected_index) {
            format!(
                "Type: {}\nDescription: {}\nObject ID: {}\nValue ID: {}",
                node.widget_runtime_type.as_deref().unwrap_or("Unknown"),
                node.description.as_deref().unwrap_or("-"),
                node.object_id.as_deref().unwrap_or("-"),
                node.value_id.as_deref().unwrap_or("-")
            )
        } else {
            "No node selected".to_string()
        }
    } else {
        "No data".to_string()
    };

    let paragraph = Paragraph::new(content).block(block);
    f.render_widget(paragraph, area);
}

// Duplicate helper for now, should move to shared util or AppState
use crate::vm_service::RemoteDiagnosticsNode;
fn flatten_tree<'a>(
    node: &'a RemoteDiagnosticsNode,
    depth: usize,
    lines: &mut Vec<String>,
    nodes: &mut Vec<&'a RemoteDiagnosticsNode>,
) {
    let _ = lines; // unused here
    nodes.push(node);
    if let Some(children) = &node.children {
        for child in children {
            flatten_tree(child, depth + 1, lines, nodes);
        }
    }
}
