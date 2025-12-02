use crate::app_state::AppState;
use crate::vm_service::RemoteDiagnosticsNode;
use ratatui::{
    layout::Rect,
    style::{Color, Style},
    widgets::{Block, Borders},
    Frame,
};

pub fn draw(f: &mut Frame, area: Rect, state: &AppState) {
    let border_style = if state.focus == crate::app_state::Focus::Tree {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default()
    };
    let block = Block::default()
        .title("Widget Tree")
        .borders(Borders::ALL)
        .border_style(border_style);
    f.render_widget(block, area);

    let inner_area = area.inner(ratatui::layout::Margin {
        vertical: 1,
        horizontal: 1,
    });

    if let Some(root) = &state.root_node {
        let mut lines = Vec::new();
        let mut visible_nodes = Vec::new();
        flatten_tree(root, 0, &mut lines, &mut visible_nodes, &state.expanded_ids);

        // Apply scrolling
        let skip_count = state.tree_scroll_offset;

        for (i, line) in lines.iter().skip(skip_count).enumerate() {
            if i >= inner_area.height as usize {
                break;
            }

            let actual_index = i + skip_count;
            let style = if actual_index == state.selected_index {
                Style::default().bg(Color::Blue).fg(Color::White)
            } else {
                Style::default()
            };

            // Apply horizontal scrolling
            let line_width = unicode_width::UnicodeWidthStr::width(line.as_str());
            let visible_width = inner_area.width as usize;
            let scroll_offset = state.tree_horizontal_scroll;

            let display_line = if scroll_offset >= line_width {
                ""
            } else {
                let mut current_width = 0;
                let mut start_byte = 0;
                let mut end_byte = line.len();
                let mut found_start = false;

                for (i, c) in line.char_indices() {
                    let char_width = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);

                    if !found_start {
                        if current_width + char_width > scroll_offset {
                            start_byte = i;
                            found_start = true;
                            // Reset width to count visible part
                            current_width = 0;
                        } else {
                            current_width += char_width;
                            continue;
                        }
                    }

                    if found_start {
                        if current_width + char_width > visible_width {
                            end_byte = i;
                            break;
                        }
                        current_width += char_width;
                    }
                }

                if !found_start {
                    ""
                } else {
                    &line[start_byte..end_byte]
                }
            };

            f.buffer_mut()
                .set_string(inner_area.x, inner_area.y + i as u16, display_line, style);
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
    expanded_ids: &std::collections::HashSet<String>,
) {
    let indent = "  ".repeat(depth);
    let description = node.description.as_deref().unwrap_or("?");
    let type_name = node
        .widget_runtime_type
        .as_deref()
        .unwrap_or(node.node_type.as_deref().unwrap_or("Unknown"));

    // Determine icon
    let has_children = node
        .children
        .as_ref()
        .map(|c| !c.is_empty())
        .unwrap_or(false);
    let is_expanded = if let Some(id) = node.value_id.as_ref().or(node.object_id.as_ref()) {
        expanded_ids.contains(id)
    } else {
        true // Default to expanded if no ID? Or collapsed? Let's say expanded for now to be safe.
    };

    let icon = if has_children {
        if is_expanded {
            "▼ "
        } else {
            "▶ "
        }
    } else {
        "  " // Placeholder for alignment
    };

    lines.push(format!(
        "{}{}{}{} ({})",
        indent, icon, type_name, "", description
    ));
    nodes.push(node);

    if has_children && is_expanded {
        if let Some(children) = &node.children {
            for child in children {
                flatten_tree(child, depth + 1, lines, nodes, expanded_ids);
            }
        }
    }
}
