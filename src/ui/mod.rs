pub mod details;
pub mod tree;

use crate::app_state::AppState;
use ratatui::{
    layout::{Constraint, Direction, Layout},
    Frame,
};

pub fn draw(f: &mut Frame, state: &AppState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(70), Constraint::Percentage(30)])
        .split(f.area());

    let main_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(chunks[0]);

    // Left: Widget Tree
    tree::draw(f, main_chunks[0], state);

    // Right: Details
    details::draw(f, main_chunks[1], state);

    // Bottom: Logs
    let border_style = if state.focus == crate::app_state::Focus::Logs {
        ratatui::style::Style::default().fg(ratatui::style::Color::Yellow)
    } else {
        ratatui::style::Style::default()
    };

    let log_block = ratatui::widgets::Block::default()
        .title("Logs")
        .borders(ratatui::widgets::Borders::ALL)
        .border_style(border_style);
    let log_area = chunks[1];
    let log_height = log_area.height as usize;

    // Calculate scroll offset
    let scroll_offset = if state.log_auto_scroll {
        state
            .logs
            .len()
            .saturating_sub(log_height.saturating_sub(2)) // -2 for borders
    } else {
        state.log_scroll_state
    };

    // Ensure scroll_offset is valid
    let scroll_offset = scroll_offset.min(state.logs.len().saturating_sub(1));

    let logs: Vec<ratatui::widgets::ListItem> = state
        .logs
        .iter()
        .skip(scroll_offset)
        .take(log_height.saturating_sub(2))
        .map(|s| ratatui::widgets::ListItem::new(ratatui::text::Line::from(s.as_str())))
        .collect();

    let logs_list = ratatui::widgets::List::new(logs).block(log_block);
    f.render_widget(logs_list, log_area);
}
