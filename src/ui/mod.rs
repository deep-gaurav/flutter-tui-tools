pub mod debugger;
pub mod details;
pub mod tree;

use crate::app_state::{AppState, Tab};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};

pub fn draw(f: &mut Frame, state: &AppState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // App Bar
            Constraint::Min(0),    // Main Content
            if state.show_logs {
                Constraint::Length(10)
            } else {
                Constraint::Length(0)
            }, // Logs
        ])
        .split(f.area());

    // App Bar
    let app_bar_block = Block::default().borders(Borders::ALL).title("Controls");
    let app_bar_area = chunks[0];

    f.render_widget(app_bar_block, app_bar_area);

    let button_titles = [
        "Inspector (1)",
        "Debugger (2)",
        "Hot Reload (r)",
        "Hot Restart (R)",
        "Auto (a)",
        "Refresh (F5)",
        "Logs (l)",
        "Quit (q)",
    ];
    for (i, title) in button_titles.iter().enumerate() {
        let button_style = if i == 4 {
            // Auto Toggle
            if state.auto_reload {
                Style::default().fg(Color::Green).bg(Color::Black)
            } else {
                Style::default().fg(Color::Red).bg(Color::Black)
            }
        } else if i == 0 && state.current_tab == Tab::Inspector {
            Style::default().fg(Color::Yellow).bg(Color::Black)
        } else if i == 1 && state.current_tab == Tab::Debugger {
            Style::default().fg(Color::Yellow).bg(Color::Black)
        } else if i == 6 {
            if state.show_logs {
                Style::default().fg(Color::Green).bg(Color::Black)
            } else {
                Style::default().fg(Color::Red).bg(Color::Black)
            }
        } else {
            Style::default().fg(Color::Cyan).bg(Color::Black)
        };

        let display_title = if i == 4 {
            if state.auto_reload {
                "Auto (a): ON"
            } else {
                "Auto (a): OFF"
            }
        } else if i == 6 {
            if state.show_logs {
                "Logs (l): ON"
            } else {
                "Logs (l): OFF"
            }
        } else {
            title
        };

        let button = Paragraph::new(display_title)
            .style(button_style)
            .alignment(ratatui::layout::Alignment::Center)
            .block(Block::default().borders(Borders::ALL));
        f.render_widget(
            button,
            Rect {
                x: app_bar_area.x + (i as u16 * 20),
                y: app_bar_area.y,
                width: 20,
                height: 3,
            },
        );
    }

    let main_area = chunks[1];

    match state.current_tab {
        Tab::Inspector => {
            let main_chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(75), Constraint::Percentage(25)])
                .split(main_area);

            // Left: Widget Tree
            state.inspector_tree_area.replace(main_chunks[0]);
            state
                .inspector_tree_height
                .replace(main_chunks[0].height as usize);
            let count = tree::draw(
                f,
                main_chunks[0],
                state.root_node.as_ref(),
                state.selected_index,
                &state.expanded_ids,
                state.tree_scroll_offset,
                state.tree_horizontal_scroll,
                "Widget Tree",
                state.focus == crate::app_state::Focus::Tree
                    || state.focus == crate::app_state::Focus::Search,
            );
            state.inspector_visible_count.replace(count);

            // Right: Details
            details::draw(f, main_chunks[1], state);
        }
        Tab::Debugger => {
            debugger::draw(f, main_area, state);
        }
    }

    // Bottom: Logs
    if state.show_logs {
        let border_style = if state.focus == crate::app_state::Focus::Logs {
            ratatui::style::Style::default().fg(ratatui::style::Color::Yellow)
        } else {
            ratatui::style::Style::default()
        };

        let log_block = ratatui::widgets::Block::default()
            .title("Logs")
            .borders(ratatui::widgets::Borders::ALL)
            .border_style(border_style);
        let log_area = chunks[2];
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

    // Isolate Selection Popup
    if state.show_isolate_selection {
        draw_isolate_selection_popup(f, state);
    }

    // Draw Search Input if active
    if state.focus == crate::app_state::Focus::Search {
        let area = centered_rect(60, 20, f.area());
        let block = Block::default()
            .title("Search")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Yellow));

        let text = format!(
            "Query: {}\nMatches: {}/{}\n\n(Enter: Next, Shift+Enter: Prev, Esc: Cancel)",
            state.search_query,
            if state.search_results.is_empty() {
                0
            } else {
                state.current_match_index + 1
            },
            state.search_results.len()
        );
        let paragraph = Paragraph::new(text).block(block);
        f.render_widget(Clear, area); // Clear background
        f.render_widget(paragraph, area);
    }
}

fn draw_isolate_selection_popup(f: &mut Frame, state: &AppState) {
    let area = centered_rect(60, 40, f.area());
    let block = ratatui::widgets::Block::default()
        .title("Select Isolate")
        .borders(ratatui::widgets::Borders::ALL)
        .style(ratatui::style::Style::default().bg(ratatui::style::Color::DarkGray));

    f.render_widget(ratatui::widgets::Clear, area); // Clear background
    f.render_widget(block.clone(), area);

    let items: Vec<ratatui::widgets::ListItem> = state
        .available_isolates
        .iter()
        .map(|iso| {
            let content = format!("{} ({})", iso.name, iso.id);
            ratatui::widgets::ListItem::new(content)
        })
        .collect();

    let list = ratatui::widgets::List::new(items)
        .block(ratatui::widgets::Block::default().borders(ratatui::widgets::Borders::NONE))
        .highlight_style(
            ratatui::style::Style::default()
                .fg(ratatui::style::Color::Black)
                .bg(ratatui::style::Color::White),
        )
        .highlight_symbol(">> ");

    let mut list_state = ratatui::widgets::ListState::default();
    list_state.select(Some(state.selected_isolate_index));

    let inner_area = block.inner(area);
    f.render_stateful_widget(list, inner_area, &mut list_state);
}

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}
