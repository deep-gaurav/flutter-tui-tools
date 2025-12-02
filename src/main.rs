mod app_state;
mod flutter_daemon;
mod logger;
mod ui;
mod vm_service;

use anyhow::Result;
use app_state::AppState;
use clap::Parser;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use flutter_daemon::FlutterDaemon;
use ratatui::{backend::CrosstermBackend, Terminal};
use std::{io, time::Duration};
use tokio::sync::mpsc;
use vm_service::VmServiceClient;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// Path to the Flutter application directory
    #[arg(short, long, default_value = ".")]
    app_dir: String,

    /// Device ID to attach to
    #[arg(short, long)]
    device_id: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Create app state
    let mut app_state = AppState::new();
    let (tx_uri, mut rx_uri) = mpsc::channel(1);
    let (tx_tree, mut rx_tree) = mpsc::channel(1);
    let (tx_log, mut rx_log) = mpsc::unbounded_channel();
    let (tx_isolates, mut rx_isolates) = mpsc::channel(1);
    let (tx_selected_isolate, mut rx_selected_isolate) = mpsc::channel(1);

    // Init logger
    logger::init(tx_log)?;

    // Start Flutter Daemon
    let daemon = FlutterDaemon::new(tx_uri);
    let app_dir = args.app_dir.clone();
    let device_id = args.device_id.clone();

    tokio::spawn(async move {
        if let Err(e) = daemon.run(&app_dir, device_id.as_deref()).await {
            log::error!("Flutter daemon error: {}", e);
        }
    });

    // VM Service Task
    tokio::spawn(async move {
        if let Some(uri) = rx_uri.recv().await {
            log::info!("Connected to VM Service at: {}", uri);
            // Connect and fetch tree
            if let Ok(mut client) = VmServiceClient::connect(&uri).await {
                log::info!("VM Service Client connected");
                if let Ok(vm) = client.get_vm().await {
                    log::info!("VM: {:?}", vm);

                    // Send isolates to UI
                    if !vm.isolates.is_empty() {
                        let _ = tx_isolates.send(vm.isolates.clone()).await;

                        // Wait for selection
                        while let Some(selected_id) = rx_selected_isolate.recv().await {
                            if let Some(isolate_ref) =
                                vm.isolates.iter().find(|i| i.id == selected_id)
                            {
                                log::info!("Checking isolate: {}", isolate_ref.name);
                                // Poll for extension
                                loop {
                                    if let Ok(isolate) = client.get_isolate(&isolate_ref.id).await {
                                        if let Some(rpcs) = isolate.extension_rpcs {
                                            if rpcs.contains(
                                                &"ext.flutter.inspector.getRootWidgetSummaryTree"
                                                    .to_string(),
                                            ) {
                                                log::info!("Inspector extension found!");
                                                break;
                                            }
                                        }
                                    }
                                    log::info!("Waiting for inspector extension...");
                                    tokio::time::sleep(Duration::from_secs(1)).await;
                                }

                                match client
                                    .get_root_widget_summary_tree("tui_inspector", &isolate_ref.id)
                                    .await
                                {
                                    Ok(tree) => {
                                        log::info!(
                                            "Root Widget fetched: {:?}",
                                            tree.widget_runtime_type
                                        );
                                        let _ = tx_tree.send(tree).await;
                                        // Success, we can break or stay connected for updates?
                                        // For now, if we want to allow re-selection on failure, we need to handle failure.
                                        // But here we succeeded.
                                    }
                                    Err(e) => {
                                        log::error!("Failed to fetch tree: {}", e);
                                        // If failed, maybe we should ask user to select again?
                                        // Send isolates again to trigger popup?
                                        let _ = tx_isolates.send(vm.isolates.clone()).await;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    });

    // Main Loop
    loop {
        // Update state from channels
        if let Ok(tree) = rx_tree.try_recv() {
            app_state.set_root_node(tree);
            app_state.connection_status = "Connected".to_string();
        }

        if let Ok(isolates) = rx_isolates.try_recv() {
            app_state.available_isolates = isolates;
            if app_state.available_isolates.len() > 1 {
                app_state.show_isolate_selection = true;
                app_state.focus = app_state::Focus::IsolateSelection;
            } else if let Some(first) = app_state.available_isolates.first() {
                // Auto-select if only one
                let _ = tx_selected_isolate.send(first.id.clone()).await;
            }
        }

        while let Ok(log_entry) = rx_log.try_recv() {
            app_state.add_log(log_entry);
        }

        terminal.draw(|f| ui::draw(f, &app_state))?;

        if crossterm::event::poll(Duration::from_millis(100))? {
            match event::read()? {
                Event::Key(key) => {
                    if app_state.show_isolate_selection {
                        match key.code {
                            KeyCode::Char('q') => break,
                            KeyCode::Up => app_state.move_isolate_selection(-1),
                            KeyCode::Down => app_state.move_isolate_selection(1),
                            KeyCode::Enter => {
                                if let Some(isolate) = app_state
                                    .available_isolates
                                    .get(app_state.selected_isolate_index)
                                {
                                    let _ = tx_selected_isolate.send(isolate.id.clone()).await;
                                    app_state.show_isolate_selection = false;
                                    app_state.focus = app_state::Focus::Tree;
                                }
                            }
                            _ => {}
                        }
                    } else {
                        match key.code {
                            KeyCode::Char('q') => break,
                            KeyCode::Tab => app_state.cycle_focus(),
                            KeyCode::Up => match app_state.focus {
                                app_state::Focus::Tree => {
                                    app_state.move_selection(-1);
                                    let (cols, rows) = terminal
                                        .size()
                                        .map(|r| (r.width, r.height))
                                        .unwrap_or((0, 0));
                                    let tree_height = (rows as f32 * 0.7) as usize; // Approx tree height
                                    let tree_width = (cols as f32 * 0.75) as usize;
                                    app_state.update_tree_scroll(tree_height.saturating_sub(2));
                                    app_state
                                        .ensure_horizontal_visibility(tree_width.saturating_sub(2));
                                }
                                app_state::Focus::Logs => app_state.scroll_logs(-1),
                                _ => {}
                            },
                            KeyCode::Down => match app_state.focus {
                                app_state::Focus::Tree => {
                                    app_state.move_selection(1);
                                    let (cols, rows) = terminal
                                        .size()
                                        .map(|r| (r.width, r.height))
                                        .unwrap_or((0, 0));
                                    let tree_height = (rows as f32 * 0.7) as usize; // Approx tree height
                                    let tree_width = (cols as f32 * 0.75) as usize;
                                    app_state.update_tree_scroll(tree_height.saturating_sub(2));
                                    app_state
                                        .ensure_horizontal_visibility(tree_width.saturating_sub(2));
                                }
                                app_state::Focus::Logs => app_state.scroll_logs(1),
                                _ => {}
                            },
                            KeyCode::Left => {
                                if app_state.focus == app_state::Focus::Tree {
                                    if key.modifiers.contains(event::KeyModifiers::SHIFT) {
                                        app_state.scroll_tree_horizontal(-1);
                                    } else if !app_state.collapse_selected() {
                                        app_state.select_parent();
                                        let (cols, rows) = terminal
                                            .size()
                                            .map(|r| (r.width, r.height))
                                            .unwrap_or((0, 0));
                                        let tree_height = (rows as f32 * 0.7) as usize;
                                        let tree_width = (cols as f32 * 0.75) as usize;
                                        app_state.update_tree_scroll(tree_height.saturating_sub(2));
                                        app_state.ensure_horizontal_visibility(
                                            tree_width.saturating_sub(2),
                                        );
                                    }
                                }
                            }
                            KeyCode::Right => {
                                if app_state.focus == app_state::Focus::Tree {
                                    if key.modifiers.contains(event::KeyModifiers::SHIFT) {
                                        app_state.scroll_tree_horizontal(1);
                                    } else {
                                        app_state.expand_selected();
                                    }
                                }
                            }
                            KeyCode::Enter | KeyCode::Char(' ') => {
                                if app_state.focus == app_state::Focus::Tree {
                                    app_state.toggle_expand();
                                }
                            }
                            KeyCode::PageUp => app_state.scroll_logs(-10),
                            KeyCode::PageDown => app_state.scroll_logs(10),
                            _ => {}
                        }
                    }
                }
                Event::Mouse(mouse) => {
                    if !app_state.show_isolate_selection {
                        match mouse.kind {
                            event::MouseEventKind::Down(event::MouseButton::Left) => {
                                let (cols, rows) = terminal
                                    .size()
                                    .map(|r| (r.width, r.height))
                                    .unwrap_or((0, 0));
                                // Layout:
                                // Vertical: 70% Top, 30% Bottom
                                // Top: 50% Left, 50% Right
                                let split_y = (rows as f32 * 0.7) as u16;
                                let split_x = (cols as f32 * 0.5) as u16;

                                if mouse.row < split_y {
                                    if mouse.column < split_x {
                                        app_state.focus = app_state::Focus::Tree;
                                        // Handle tree click
                                        let tree_y = mouse.row.saturating_sub(1); // -1 for top border
                                        if tree_y < split_y.saturating_sub(2) {
                                            // Check if within content area
                                            let clicked_index =
                                                tree_y as usize + app_state.tree_scroll_offset;
                                            if clicked_index < app_state.visible_count() {
                                                app_state.selected_index = clicked_index;
                                            }
                                        }
                                    } else {
                                        app_state.focus = app_state::Focus::Details;
                                    }
                                } else {
                                    app_state.focus = app_state::Focus::Logs;
                                }
                            }
                            event::MouseEventKind::ScrollDown => {
                                let (cols, rows) = terminal
                                    .size()
                                    .map(|r| (r.width, r.height))
                                    .unwrap_or((0, 0));
                                let split_y = (rows as f32 * 0.7) as u16;
                                let split_x = (cols as f32 * 0.5) as u16;

                                if mouse.row >= split_y {
                                    app_state.scroll_logs(1);
                                } else if mouse.row < split_y && mouse.column < split_x {
                                    app_state.scroll_tree(1);
                                }
                            }
                            event::MouseEventKind::ScrollUp => {
                                let (cols, rows) = terminal
                                    .size()
                                    .map(|r| (r.width, r.height))
                                    .unwrap_or((0, 0));
                                let split_y = (rows as f32 * 0.7) as u16;
                                let split_x = (cols as f32 * 0.5) as u16;

                                if mouse.row >= split_y {
                                    app_state.scroll_logs(-1);
                                } else if mouse.row < split_y && mouse.column < split_x {
                                    app_state.scroll_tree(-1);
                                }
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
    }

    // Restore terminal
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    Ok(())
}
