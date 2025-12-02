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
    let logger = tui_logger::TuiLoggerWidget::default()
        .block(
            ratatui::widgets::Block::default()
                .title("Logs")
                .borders(ratatui::widgets::Borders::ALL),
        )
        .style_error(ratatui::style::Style::default().fg(ratatui::style::Color::Red))
        .style_warn(ratatui::style::Style::default().fg(ratatui::style::Color::Yellow))
        .style_info(ratatui::style::Style::default().fg(ratatui::style::Color::Cyan));
    f.render_widget(logger, chunks[1]);
}
