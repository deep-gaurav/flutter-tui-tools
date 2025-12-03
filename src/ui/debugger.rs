use crate::app_state::AppState;
use ratatui::{
    layout::{Constraint, Direction, Layout},
    style::{Color, Style},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

pub fn draw(f: &mut Frame, area: ratatui::layout::Rect, state: &AppState) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(20), // File Explorer
            Constraint::Percentage(50), // Source Code
            Constraint::Percentage(30), // Breakpoints/Stack
        ])
        .split(area);

    // File Explorer
    state.debugger_tree_area.replace(chunks[0]);
    state
        .debugger_tree_height
        .replace(chunks[0].height as usize);
    let count = crate::ui::tree::draw(
        f,
        chunks[0],
        state.file_tree.as_ref(),
        state.debugger_selected_index,
        &state.debugger_expanded_ids,
        state.debugger_tree_scroll_offset,
        state.debugger_tree_horizontal_scroll,
        "Files",
        state.focus == crate::app_state::Focus::DebuggerFiles,
    );
    state.debugger_visible_count.replace(count);

    // Search Bar (Overlay or Bottom of File Explorer)
    if state.focus == crate::app_state::Focus::DebuggerSearch
        || !state.debugger_search_query.is_empty()
    {
        let search_area = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(0), Constraint::Length(3)])
            .split(chunks[0])[1];

        // Clear area for search bar to avoid overlap with tree
        f.render_widget(ratatui::widgets::Clear, search_area);

        let search_text = if state.debugger_search_results.is_empty() {
            state.debugger_search_query.clone()
        } else {
            format!(
                "{} ({}/{})",
                state.debugger_search_query,
                state.debugger_current_match_index + 1,
                state.debugger_search_results.len()
            )
        };

        let search_block = Block::default()
            .title("Search Files")
            .borders(Borders::ALL)
            .border_style(if state.focus == crate::app_state::Focus::DebuggerSearch {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default()
            });

        let p = Paragraph::new(search_text).block(search_block);
        f.render_widget(p, search_area);
    }

    // Source Code
    state.debugger_source_area.replace(chunks[1]);
    let source_block = Block::default().title("Source Code").borders(Borders::ALL);
    let source_area = chunks[1];
    f.render_widget(source_block.clone(), source_area);

    let inner_source_area = source_block.inner(source_area);

    if let Some(content) = &state.open_file_content {
        // Simple rendering for now: line numbers + content
        let lines: Vec<ratatui::widgets::ListItem> = content
            .iter()
            .enumerate()
            .skip(state.source_scroll_offset)
            .take(inner_source_area.height as usize)
            .map(|(i, line)| {
                let line_num = i + 1;
                // Check if breakpoint exists
                let path = state.open_file_path.as_deref().unwrap_or("");
                let bp_key = format!("{}:{}", path, line_num);
                let is_bp = state.breakpoints.contains(&bp_key);

                let is_selected = state.source_selected_line == Some(i);

                let prefix = if is_bp { "●" } else { " " };
                let mut style = Style::default();
                if is_bp {
                    style = style.fg(Color::Red);
                }
                if is_selected {
                    style = style.bg(Color::DarkGray);
                }

                ratatui::widgets::ListItem::new(ratatui::text::Line::from(vec![
                    ratatui::text::Span::styled(format!("{} {:4} ", prefix, line_num), style),
                    ratatui::text::Span::raw(line),
                ]))
            })
            .collect();

        let list = ratatui::widgets::List::new(lines);
        f.render_widget(list, inner_source_area);
    } else {
        let p = Paragraph::new("No file open").alignment(ratatui::layout::Alignment::Center);
        f.render_widget(p, inner_source_area);
    }

    // Right Panel
    let right_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(chunks[2]);

    let breakpoints_list: Vec<ratatui::widgets::ListItem> = state
        .breakpoints
        .iter()
        .map(|bp| ratatui::widgets::ListItem::new(bp.as_str()))
        .collect();

    let breakpoints = ratatui::widgets::List::new(breakpoints_list)
        .block(Block::default().title("Breakpoints").borders(Borders::ALL));
    f.render_widget(breakpoints, right_chunks[0]);

    let mut stack_items = Vec::new();
    match &state.debug_state {
        crate::app_state::DebugState::Paused { reason, .. } => {
            stack_items.push(ratatui::widgets::ListItem::new(format!(
                "Paused: {}",
                reason
            )));
            if let Some(stack) = &state.stack_trace {
                if let Some(frames) = stack.get("frames").and_then(|f| f.as_array()) {
                    for frame in frames {
                        if let Some(func) = frame
                            .get("function")
                            .and_then(|f| f.get("name"))
                            .and_then(|n| n.as_str())
                        {
                            stack_items
                                .push(ratatui::widgets::ListItem::new(format!("- {}", func)));
                        }
                    }
                }
            }
        }
        crate::app_state::DebugState::Running => {
            stack_items.push(ratatui::widgets::ListItem::new("Running..."));
        }
    };

    let stack_list = ratatui::widgets::List::new(stack_items)
        .block(Block::default().title("Call Stack").borders(Borders::ALL));
    f.render_widget(stack_list, right_chunks[1]);
}
