use crate::app_state::AppState;
use ratatui::{
    layout::Rect,
    widgets::{Block, Borders, Paragraph},
    Frame,
};

pub fn draw(f: &mut Frame, area: Rect, state: &AppState) {
    let border_style = if state.focus == crate::app_state::Focus::Details {
        ratatui::style::Style::default().fg(ratatui::style::Color::Yellow)
    } else {
        ratatui::style::Style::default()
    };
    let block = Block::default()
        .title("Details")
        .borders(Borders::ALL)
        .border_style(border_style);

    let content = if let Some(details) = &state.selected_node_details {
        let mut text = format!(
            "Type: {}\nDescription: {}\nObject ID: {}\nValue ID: {}\n\nProperties:\n",
            details.widget_runtime_type.as_deref().unwrap_or("Unknown"),
            details.description.as_deref().unwrap_or("-"),
            details.object_id.as_deref().unwrap_or("-"),
            details.value_id.as_deref().unwrap_or("-")
        );

        if let Some(props) = &details.properties {
            for prop in props {
                let name = prop.name.as_deref().unwrap_or("");
                let desc = prop.description.as_deref().unwrap_or("");
                if !name.is_empty() || !desc.is_empty() {
                    text.push_str(&format!("- {}: {}\n", name, desc));
                }
            }
        }
        text
    } else if let Some(root) = &state.root_node {
        // Fallback to tree node if details not yet loaded
        // ... (existing logic)
        let mut lines = Vec::new();
        let mut visible_nodes = Vec::new();
        flatten_tree(root, 0, &mut lines, &mut visible_nodes);

        if let Some(node) = visible_nodes.get(state.selected_index) {
            format!(
                "Type: {}\nDescription: {}\nObject ID: {}\nValue ID: {}\n\n(Fetching details...)",
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
