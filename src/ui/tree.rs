use ratatui::{
    layout::Rect,
    style::{Color, Style},
    widgets::{Block, Borders},
    Frame,
};
use std::collections::HashSet;

pub trait Treeable: Sized {
    fn children(&self) -> Option<&[Self]>;
    fn id(&self) -> Option<&str>;
    fn render(&self, depth: usize, is_expanded: bool) -> String;
}

pub fn draw<T: Treeable>(
    f: &mut Frame,
    area: Rect,
    root_node: Option<&T>,
    selected_index: usize,
    expanded_ids: &HashSet<String>,
    scroll_offset: usize,
    horizontal_scroll: usize,
    title: &str,
    is_focused: bool,
) -> usize {
    let mut lines = Vec::new();
    if let Some(root) = root_node {
        flatten_tree(root, 0, &mut lines, expanded_ids);
    }

    let visible_count = lines.len();

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(if is_focused {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default()
        });

    let inner_area = block.inner(area);
    f.render_widget(block, area);

    if lines.is_empty() {
        if root_node.is_none() {
            f.buffer_mut().set_string(
                inner_area.x,
                inner_area.y,
                "Waiting for data...",
                Style::default().fg(Color::Yellow),
            );
        }
        return 0;
    }

    // Apply scrolling
    let skip_count = scroll_offset;

    for (i, line) in lines.iter().skip(skip_count).enumerate() {
        if i >= inner_area.height as usize {
            break;
        }

        let actual_index = i + skip_count;
        let style = if actual_index == selected_index {
            Style::default().bg(Color::Blue).fg(Color::White)
        } else {
            Style::default()
        };

        // Apply horizontal scrolling
        let line_width = unicode_width::UnicodeWidthStr::width(line.as_str());
        let visible_width = inner_area.width as usize;
        let scroll_offset = horizontal_scroll;

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

    visible_count
}

fn flatten_tree<T: Treeable>(
    node: &T,
    depth: usize,
    lines: &mut Vec<String>,
    expanded_ids: &HashSet<String>,
) {
    let has_children = node.children().map(|c| !c.is_empty()).unwrap_or(false);
    let is_expanded = if let Some(id) = node.id() {
        expanded_ids.contains(id)
    } else {
        true // Default expanded if no ID?
    };

    lines.push(node.render(depth, is_expanded));

    if has_children && is_expanded {
        if let Some(children) = node.children() {
            for child in children {
                flatten_tree(child, depth + 1, lines, expanded_ids);
            }
        }
    }
}

// Implement Treeable for RemoteDiagnosticsNode
impl Treeable for crate::vm_service::RemoteDiagnosticsNode {
    fn children(&self) -> Option<&[Self]> {
        self.children.as_deref()
    }

    fn id(&self) -> Option<&str> {
        self.value_id.as_deref().or(self.object_id.as_deref())
    }

    fn render(&self, depth: usize, is_expanded: bool) -> String {
        let indent = "  ".repeat(depth);
        let description = self.description.as_deref().unwrap_or("?");
        let type_name = self
            .widget_runtime_type
            .as_deref()
            .unwrap_or(self.node_type.as_deref().unwrap_or("Unknown"));

        let has_children = self
            .children
            .as_ref()
            .map(|c| !c.is_empty())
            .unwrap_or(false);

        let icon = if has_children {
            if is_expanded {
                "▼ "
            } else {
                "▶ "
            }
        } else {
            "  "
        };

        format!("{}{}{}{} ({})", indent, icon, type_name, "", description)
    }
}

pub fn count_visible_nodes<T: Treeable>(node: &T, expanded_ids: &HashSet<String>) -> usize {
    let mut count = 1; // Count self
    if let Some(id) = node.id() {
        if expanded_ids.contains(id) {
            if let Some(children) = node.children() {
                for child in children {
                    count += count_visible_nodes(child, expanded_ids);
                }
            }
        }
    }
    count
}

pub fn get_node_at_index<'a, T: Treeable>(
    node: &'a T,
    expanded_ids: &HashSet<String>,
    target_index: usize,
    current_index: &mut usize,
) -> Option<&'a T> {
    if *current_index == target_index {
        return Some(node);
    }
    *current_index += 1;

    if let Some(id) = node.id() {
        if expanded_ids.contains(id) {
            if let Some(children) = node.children() {
                for child in children {
                    if let Some(found) =
                        get_node_at_index(child, expanded_ids, target_index, current_index)
                    {
                        return Some(found);
                    }
                }
            }
        }
    }
    None
}
