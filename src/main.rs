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
                    if let Some(isolate_ref) = vm.isolates.first() {
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
                                log::info!("Root Widget fetched: {:?}", tree.widget_runtime_type);
                                let _ = tx_tree.send(tree).await;
                            }
                            Err(e) => {
                                log::error!("Failed to fetch tree: {}", e);
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

        while let Ok(log_entry) = rx_log.try_recv() {
            app_state.add_log(log_entry);
        }

        terminal.draw(|f| ui::draw(f, &app_state))?;

        if crossterm::event::poll(Duration::from_millis(100))? {
            match event::read()? {
                Event::Key(key) => match key.code {
                    KeyCode::Char('q') => break,
                    KeyCode::Tab => app_state.cycle_focus(),
                    KeyCode::Up => match app_state.focus {
                        app_state::Focus::Tree => {
                            app_state.move_selection(-1);
                            let (_, rows) = terminal
                                .size()
                                .map(|r| (r.width, r.height))
                                .unwrap_or((0, 0));
                            let tree_height = (rows as f32 * 0.7) as usize; // Approx tree height
                            app_state.update_tree_scroll(tree_height.saturating_sub(2));
                            // -2 for borders
                        }
                        app_state::Focus::Logs => app_state.scroll_logs(-1),
                        _ => {}
                    },
                    KeyCode::Down => match app_state.focus {
                        app_state::Focus::Tree => {
                            app_state.move_selection(1);
                            let (_, rows) = terminal
                                .size()
                                .map(|r| (r.width, r.height))
                                .unwrap_or((0, 0));
                            let tree_height = (rows as f32 * 0.7) as usize; // Approx tree height
                            app_state.update_tree_scroll(tree_height.saturating_sub(2));
                        }
                        app_state::Focus::Logs => app_state.scroll_logs(1),
                        _ => {}
                    },
                    KeyCode::Left => {
                        if app_state.focus == app_state::Focus::Tree {
                            if !app_state.collapse_selected() {
                                app_state.select_parent();
                                let (_, rows) = terminal
                                    .size()
                                    .map(|r| (r.width, r.height))
                                    .unwrap_or((0, 0));
                                let tree_height = (rows as f32 * 0.7) as usize;
                                app_state.update_tree_scroll(tree_height.saturating_sub(2));
                            }
                        }
                    }
                    KeyCode::Right => {
                        if app_state.focus == app_state::Focus::Tree {
                            app_state.expand_selected();
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
                },
                Event::Mouse(mouse) => {
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
                                            // Check if clicked on arrow (approximate check)
                                            // We don't know exact indentation here easily without re-calculating.
                                            // But we can just toggle if double clicked or something?
                                            // Or just toggle if clicked?
                                            // Let's just select for now.
                                            // If we want to support click-to-toggle, we'd need to know if the click was on the arrow.
                                            // For now, let's just make click select.
                                            // Maybe double click to toggle? MouseEvent doesn't give double click easily.
                                            // Let's stick to selection on click.
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
