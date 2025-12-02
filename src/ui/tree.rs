use crate::app_state::AppState;
use crate::vm_service::RemoteDiagnosticsNode;
use ratatui::{
    layout::Rect,
    style::{Color, Style},
    widgets::{Block, Borders},
    Frame,
};

pub fn draw(f: &mut Frame, area: Rect, state: &AppState) {
    let block = Block::default().title("Widget Tree").borders(Borders::ALL);
    f.render_widget(block, area);

    let inner_area = area.inner(ratatui::layout::Margin {
        vertical: 1,
        horizontal: 1,
    });

    if let Some(root) = &state.root_node {
        let mut lines = Vec::new();
        let mut visible_nodes = Vec::new();
        flatten_tree(root, 0, &mut lines, &mut visible_nodes);

        for (i, line) in lines.iter().enumerate() {
            if i >= inner_area.height as usize {
                break;
            }

            let style = if i == state.selected_index {
                Style::default().bg(Color::Blue).fg(Color::White)
            } else {
                Style::default()
            };

            f.buffer_mut()
                .set_string(inner_area.x, inner_area.y + i as u16, line, style);
        }
    } else {
        f.buffer_mut().set_string(
            inner_area.x,
            inner_area.y,
            "Waiting for widget tree...",
            Style::default().fg(Color::Yellow),
        );
    }
}

fn flatten_tree<'a>(
    node: &'a RemoteDiagnosticsNode,
    depth: usize,
    lines: &mut Vec<String>,
    nodes: &mut Vec<&'a RemoteDiagnosticsNode>,
) {
    let indent = "  ".repeat(depth);
    let description = node.description.as_deref().unwrap_or("?");
    let type_name = node
        .widget_runtime_type
        .as_deref()
        .unwrap_or(node.node_type.as_deref().unwrap_or("Unknown"));

    lines.push(format!("{}{} ({})", indent, type_name, description));
    nodes.push(node);

    if let Some(children) = &node.children {
        for child in children {
            flatten_tree(child, depth + 1, lines, nodes);
        }
    }
}
