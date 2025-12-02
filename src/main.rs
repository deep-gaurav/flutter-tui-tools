mod app_state;
mod flutter_daemon;
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

    // Init logger
    tui_logger::init_logger(log::LevelFilter::Info).unwrap();
    tui_logger::set_default_level(log::LevelFilter::Info);

    // Create app state
    let mut app_state = AppState::new();
    let (tx_uri, mut rx_uri) = mpsc::channel(1);
    let (tx_tree, mut rx_tree) = mpsc::channel(1);

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

        terminal.draw(|f| ui::draw(f, &app_state))?;

        if crossterm::event::poll(Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                match key.code {
                    KeyCode::Char('q') => break,
                    KeyCode::Up => app_state.move_selection(-1),
                    KeyCode::Down => app_state.move_selection(1),
                    _ => {}
                }
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
