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
use ignore::gitignore::Gitignore;
use notify::{Config, RecommendedWatcher, RecursiveMode, Watcher};
use ratatui::{backend::CrosstermBackend, Terminal};
use std::path::Path;
use std::{
    io,
    time::{Duration, Instant},
};
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

    /// Directory to watch for changes (defaults to app_dir)
    #[arg(short, long)]
    watch_dir: Option<String>,
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
    let project_root = std::path::PathBuf::from(&args.app_dir)
        .canonicalize()
        .unwrap_or_else(|_| std::path::PathBuf::from(&args.app_dir));
    let mut app_state = AppState::new(project_root);
    let (tx_uri, mut rx_uri) = mpsc::channel(1);
    let (tx_tree, mut rx_tree) = mpsc::channel(1);
    let (tx_log, mut rx_log) = mpsc::unbounded_channel();
    let (tx_isolates, mut rx_isolates) = mpsc::channel::<Vec<vm_service::IsolateRef>>(1);
    let (tx_selected_isolate, mut rx_selected_isolate) = mpsc::channel::<String>(1);
    let (tx_details_request, mut rx_details_request) = mpsc::channel::<String>(1);
    let (tx_details, mut rx_details) = mpsc::channel::<vm_service::RemoteDiagnosticsNode>(1);
    let (tx_cmd, rx_cmd) = mpsc::channel::<String>(10);
    let (tx_refresh, mut rx_refresh) = mpsc::channel::<()>(1);
    let (tx_vm_client, mut rx_vm_client) = mpsc::channel::<vm_service::VmServiceClient>(1);
    let (tx_debug_event, mut rx_debug_event) =
        mpsc::channel::<(app_state::DebugState, Option<serde_json::Value>)>(10);

    app_state.tx_flutter_command = Some(tx_cmd);

    // Init logger
    logger::init(tx_log)?;

    // Setup File Watcher
    let (tx_watch, mut rx_watch) = mpsc::channel::<()>(1);
    let watch_dir = args.watch_dir.clone().unwrap_or(args.app_dir.clone());

    // We need a thread to run the watcher because notify is blocking/sync in its callback usually,
    // or we can use a channel.
    // Notify's recommended watcher uses a thread internally.
    // We'll use a standard std channel to bridge to tokio channel if needed,
    // or just spawn a blocking task.
    // Actually, we can just use a sync channel and poll it?
    // Or better, use a separate task to bridge.

    let (std_tx, std_rx) = std::sync::mpsc::channel();
    let mut watcher = RecommendedWatcher::new(std_tx, Config::default())?;

    let path_to_watch = Path::new(&watch_dir);
    log::info!(
        "Watching directory: {:?}",
        path_to_watch
            .canonicalize()
            .unwrap_or(path_to_watch.to_path_buf())
    );
    watcher.watch(path_to_watch, RecursiveMode::Recursive)?;

    // Load gitignore
    let (gitignore, _) = Gitignore::new(path_to_watch.join(".gitignore"));

    // Bridge task
    tokio::spawn(async move {
        while let Ok(res) = std_rx.recv() {
            match res {
                Ok(event) => {
                    let is_dart_change = event.paths.iter().any(|p| {
                        // Check gitignore
                        if gitignore.matched(p, false).is_ignore() {
                            return false;
                        }
                        p.extension().map_or(false, |ext| ext == "dart")
                    });

                    if is_dart_change {
                        log::info!("Dart file changed: {:?}", event.paths);
                        let _ = tx_watch.send(()).await;
                    }
                }
                Err(e) => log::error!("Watch error: {:?}", e),
            }
        }
    });

    // Start Flutter Daemon
    let daemon = FlutterDaemon::new(tx_uri);
    let app_dir = args.app_dir.clone();
    let device_id = args.device_id.clone();

    tokio::spawn(async move {
        if let Err(e) = daemon.run(&app_dir, device_id.as_deref(), rx_cmd).await {
            log::error!("Flutter daemon error: {}", e);
        }
    });

    // Populate file list and tree
    app_state.build_file_tree();

    // VM Service Task
    tokio::spawn(async move {
        if let Some(uri) = rx_uri.recv().await {
            log::info!("Connected to VM Service at: {}", uri);
            // Connect and fetch tree
            if let Ok((client, mut rx_event)) = VmServiceClient::connect(&uri).await {
                log::info!("VM Service Client connected");
                let _ = tx_vm_client.send(client.clone()).await;

                // Subscribe to streams
                if let Err(e) = client.stream_listen("Debug").await {
                    log::error!("Failed to subscribe to Debug stream: {}", e);
                } else {
                    log::info!("Subscribed to Debug stream");
                }
                if let Err(e) = client.stream_listen("Isolate").await {
                    log::error!("Failed to subscribe to Isolate stream: {}", e);
                } else {
                    log::info!("Subscribed to Isolate stream");
                }
                if let Err(e) = client.stream_listen("Extension").await {
                    log::error!("Failed to subscribe to Extension stream: {}", e);
                } else {
                    log::info!("Subscribed to Extension stream");
                }

                if let Ok(vm) = client.get_vm().await {
                    log::info!("VM fetched: isolates count = {}", vm.isolates.len());

                    // Send isolates to UI
                    if !vm.isolates.is_empty() {
                        let _ = tx_isolates.send(vm.isolates.clone()).await;

                        // Wait for selection
                        let mut current_isolate_id: Option<String> = None;
                        log::info!("Starting VM Event Loop");

                        loop {
                            tokio::select! {
                                Some(event) = rx_event.recv() => {
                                    // Handle VM Events
                                    match event.event_kind.as_str() {
                                        "PauseStart" | "PauseBreakpoint" | "PauseException" | "PauseInterrupted" | "PauseExit" => {
                                            log::info!("VM Event: {} in {:?}", event.event_kind, event.isolate_id);
                                            // Fetch stack
                                            if let Some(isolate_id) = &event.isolate_id {
                                                if let Ok(stack) = client.get_stack(isolate_id).await {
                                                    let _ = tx_debug_event.send((app_state::DebugState::Paused {
                                                        isolate_id: isolate_id.clone(),
                                                        reason: event.event_kind.clone(),
                                                    }, Some(stack))).await;
                                                }
                                            }
                                        }
                                        "Resume" => {
                                            log::info!("VM Event: Resumed");
                                            let _ = tx_debug_event.send((app_state::DebugState::Running, None)).await;
                                        }
                                        _ => {
                                            // log::debug!("VM Event: {}", event.event_kind);
                                        }
                                    }
                                }
                                Some(selected_id) = rx_selected_isolate.recv() => {
                                    log::info!("VM Task: Received selected isolate ID: {}", selected_id);
                                    if let Some(isolate_ref) = vm.isolates.iter().find(|i| i.id == selected_id) {
                                        log::info!("Checking isolate: {}", isolate_ref.name);
                                        current_isolate_id = Some(isolate_ref.id.clone());

                                        let client = client.clone();
                                        let isolate_ref = isolate_ref.clone();
                                        let tx_tree = tx_tree.clone();
                                        let tx_isolates = tx_isolates.clone();
                                        let vm_isolates = vm.isolates.clone();

                                        tokio::spawn(async move {
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
                                                    let _ = tx_isolates.send(vm_isolates).await;
                                                }
                                            }
                                        });
                                    }
                                }
                                Some(object_id) = rx_details_request.recv() => {
                                    if let Some(isolate_id) = &current_isolate_id {
                                        log::info!("VM: Fetching details for {} in isolate {}", object_id, isolate_id);
                                        match client.get_details_subtree(isolate_id, &object_id, 2).await {
                                            Ok(details) => {
                                                log::info!("VM: Details fetched successfully");
                                                let _ = tx_details.send(details).await;
                                            }
                                            Err(e) => {
                                                log::error!("VM: Failed to fetch details: {}", e);
                                            }
                                        }
                                    } else {
                                        log::warn!("VM: Received details request but current_isolate_id is None");
                                    }
                                }
                                Some(_) = rx_refresh.recv() => {
                                    log::info!("VM: Refreshing isolates and tree...");
                                    match client.get_vm().await {
                                        Ok(vm) => {
                                            log::info!("VM: Refreshed VM, isolates: {}", vm.isolates.len());
                                            let _ = tx_isolates.send(vm.isolates).await;
                                        }
                                        Err(e) => {
                                            log::error!("Failed to refresh VM: {}", e);
                                        }
                                    }
                                }
                                else => {
                                    break;
                                }
                            }
                        }
                    }
                }
            }
        }
    });

    // Main Loop
    let mut debounce_deadline: Option<Instant> = None;

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

        if let Ok(details) = rx_details.try_recv() {
            app_state.selected_node_details = Some(details);
        }

        while let Ok(log_entry) = rx_log.try_recv() {
            // Check for hot reload/restart completion
            if log_entry.contains("Reloaded") || log_entry.contains("Restarted") {
                let _ = tx_refresh.try_send(());
            }
            app_state.add_log(log_entry);
        }

        if let Ok(client) = rx_vm_client.try_recv() {
            log::info!("Main Loop: Received VM Service Client");
            app_state.vm_service_client = Some(client);
        }

        if let Ok((state, stack)) = rx_debug_event.try_recv() {
            log::info!("Main Loop: Received Debug Event: {:?}", state);
            app_state.debug_state = state;
            if let Some(stack) = stack {
                app_state.stack_trace = Some(stack);
            }
        }

        // Handle File Watcher Events
        if let Ok(_) = rx_watch.try_recv() {
            // Reset debounce timer
            debounce_deadline = Some(Instant::now() + Duration::from_millis(500));
        }

        // Check Debounce Timer
        if let Some(deadline) = debounce_deadline {
            if Instant::now() >= deadline {
                debounce_deadline = None;
                if app_state.auto_reload {
                    if let Some(tx) = &app_state.tx_flutter_command {
                        let _ = tx.send("r".to_string()).await;
                    }
                }
            }
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
                    } else if app_state.focus == app_state::Focus::Search {
                        match key.code {
                            KeyCode::Esc => {
                                app_state.focus = app_state::Focus::Tree;
                            }
                            KeyCode::Enter => {
                                if key.modifiers.contains(event::KeyModifiers::SHIFT) {
                                    app_state.prev_match();
                                } else {
                                    app_state.next_match();
                                }
                            }
                            KeyCode::Char(c) => {
                                app_state.search_query.push(c);
                                app_state.perform_search();
                            }
                            KeyCode::Backspace => {
                                app_state.search_query.pop();
                                app_state.perform_search();
                            }
                            _ => {}
                        }
                    } else if app_state.focus == app_state::Focus::DebuggerSource {
                        match key.code {
                            KeyCode::Esc => {
                                app_state.focus = app_state::Focus::DebuggerFiles;
                            }
                            KeyCode::Char('b') => {
                                if let Some(line_idx) = app_state.source_selected_line {
                                    if let Some(path) = &app_state.open_file_path {
                                        let line = line_idx + 1;
                                        let bp_id = format!("{}:{}", path, line);

                                        let is_existing = app_state.breakpoints.contains(&bp_id);
                                        if is_existing {
                                            app_state.breakpoints.remove(&bp_id);
                                            // TODO: Send removeBreakpoint to VM
                                        } else {
                                            app_state.breakpoints.insert(bp_id.clone());
                                            // Send addBreakpoint to VM
                                            if let Some(client) = &app_state.vm_service_client {
                                                let client = client.clone();
                                                if let Some(isolate) = app_state
                                                    .available_isolates
                                                    .get(app_state.selected_isolate_index)
                                                {
                                                    let isolate_id = isolate.id.clone();
                                                    let full_path =
                                                        app_state.project_root.join(path);
                                                    let script_uri = format!(
                                                        "file://{}",
                                                        full_path.to_string_lossy()
                                                    );

                                                    log::info!("Attempting to set breakpoint at {} line {}", script_uri, line);

                                                    tokio::spawn(async move {
                                                        match client
                                                            .add_breakpoint_with_script_uri(
                                                                &isolate_id,
                                                                &script_uri,
                                                                line,
                                                            )
                                                            .await
                                                        {
                                                            Ok(response) => {
                                                                log::info!(
                                                                    "Added breakpoint at {}:{}",
                                                                    script_uri,
                                                                    line
                                                                );
                                                                log::info!(
                                                                    "VM Response: {:?}",
                                                                    response
                                                                );
                                                            }
                                                            Err(e) => log::error!(
                                                                "Failed to add breakpoint: {}",
                                                                e
                                                            ),
                                                        }
                                                    });
                                                }
                                            }
                                        }
                                    }
                                } else {
                                    log::warn!("Cannot toggle breakpoint: No line selected. Please open a file and select a line.");
                                }
                            }
                            KeyCode::F(5) => {
                                // Resume
                                if let Some(client) = &app_state.vm_service_client {
                                    let client = client.clone();
                                    if let Some(isolate) = app_state
                                        .available_isolates
                                        .get(app_state.selected_isolate_index)
                                    {
                                        let isolate_id = isolate.id.clone();
                                        tokio::spawn(async move {
                                            let _ = client.resume(&isolate_id, None).await;
                                        });
                                    }
                                }
                            }
                            KeyCode::F(10) => {
                                // Step Over
                                if let Some(client) = &app_state.vm_service_client {
                                    let client = client.clone();
                                    if let Some(isolate) = app_state
                                        .available_isolates
                                        .get(app_state.selected_isolate_index)
                                    {
                                        let isolate_id = isolate.id.clone();
                                        tokio::spawn(async move {
                                            let _ = client.resume(&isolate_id, Some("Over")).await;
                                        });
                                    }
                                }
                            }
                            KeyCode::F(11) => {
                                // Step Into
                                if let Some(client) = &app_state.vm_service_client {
                                    let client = client.clone();
                                    if let Some(isolate) = app_state
                                        .available_isolates
                                        .get(app_state.selected_isolate_index)
                                    {
                                        let isolate_id = isolate.id.clone();
                                        tokio::spawn(async move {
                                            let _ = client.resume(&isolate_id, Some("Into")).await;
                                        });
                                    }
                                }
                            }
                            _ => {}
                        }
                    } else {
                        match key.code {
                            KeyCode::Char('1') => {
                                app_state.current_tab = app_state::Tab::Inspector;
                            }
                            KeyCode::Char('2') => {
                                app_state.current_tab = app_state::Tab::Debugger;
                            }
                            KeyCode::Char('l') => {
                                app_state.show_logs = !app_state.show_logs;
                            }
                            KeyCode::Char('q') => {
                                if let Some(tx) = &app_state.tx_flutter_command {
                                    let _ = tx.send("q".to_string()).await;
                                }
                                break;
                            }
                            KeyCode::Char('r') => {
                                if let Some(tx) = &app_state.tx_flutter_command {
                                    let _ = tx.send("r".to_string()).await;
                                }
                            }
                            KeyCode::Char('R') => {
                                if let Some(tx) = &app_state.tx_flutter_command {
                                    let _ = tx.send("R".to_string()).await;
                                }
                            }
                            KeyCode::Char('a') => {
                                app_state.auto_reload = !app_state.auto_reload;
                            }
                            KeyCode::Char('f') => {
                                if app_state.focus == app_state::Focus::Tree {
                                    app_state.focus_selected_node();
                                }
                            }
                            KeyCode::Char('/') => {
                                if app_state.focus == app_state::Focus::DebuggerFiles {
                                    app_state.focus = app_state::Focus::DebuggerSearch;
                                    app_state.debugger_search_query.clear();
                                } else {
                                    app_state.focus = app_state::Focus::Search;
                                    app_state.search_query.clear();
                                }
                            }
                            KeyCode::Tab => app_state.cycle_focus(),
                            KeyCode::Esc => {
                                if app_state.focus == app_state::Focus::DebuggerSearch {
                                    app_state.focus = app_state::Focus::DebuggerFiles;
                                } else if app_state.focus == app_state::Focus::Search {
                                    app_state.focus = app_state::Focus::Tree;
                                } else if app_state.focus == app_state::Focus::DebuggerSource {
                                    app_state.focus = app_state::Focus::DebuggerFiles;
                                }
                            }
                            KeyCode::Char(c)
                                if app_state.focus == app_state::Focus::DebuggerSearch =>
                            {
                                app_state.debugger_search_query.push(c);
                                app_state.perform_debugger_search();
                            }
                            KeyCode::Backspace
                                if app_state.focus == app_state::Focus::DebuggerSearch =>
                            {
                                app_state.debugger_search_query.pop();
                                app_state.perform_debugger_search();
                            }
                            KeyCode::Enter
                                if app_state.focus == app_state::Focus::DebuggerSearch =>
                            {
                                app_state.next_debugger_match();
                            }
                            KeyCode::Char('n')
                                if app_state.focus == app_state::Focus::DebuggerFiles =>
                            {
                                app_state.next_debugger_match();
                            }
                            KeyCode::Char('N')
                                if app_state.focus == app_state::Focus::DebuggerFiles =>
                            {
                                app_state.previous_debugger_match();
                            }
                            KeyCode::Up => match app_state.focus {
                                app_state::Focus::Tree => {
                                    if app_state.current_tab == app_state::Tab::Inspector {
                                        app_state.move_selection(-1);
                                        let (cols, rows) = terminal
                                            .size()
                                            .map(|r| (r.width, r.height))
                                            .unwrap_or((0, 0));
                                        let tree_height = (rows.saturating_sub(3 + 10)) as usize; // Approx tree height (minus app bar and logs)
                                        let tree_width = (cols as f32 * 0.75) as usize;
                                        app_state.update_tree_scroll(tree_height.saturating_sub(2));
                                        app_state.ensure_horizontal_visibility(
                                            tree_width.saturating_sub(2),
                                        );

                                        // Request details
                                        if let Some(node) = app_state.get_selected_node() {
                                            if let Some(id) = AppState::get_node_id(node) {
                                                log::info!("UI: Requesting details for id: {}", id);
                                                let _ = tx_details_request.try_send(id);
                                            } else {
                                                log::warn!(
                                                    "UI: Selected node has no object_id or value_id"
                                                );
                                            }
                                        } else {
                                            log::warn!("UI: No node selected");
                                        }
                                    }
                                }
                                app_state::Focus::Logs => app_state.scroll_logs(-1),
                                app_state::Focus::DebuggerFiles => {
                                    app_state.move_debugger_selection(-1);
                                    let (cols, rows) = terminal
                                        .size()
                                        .map(|r| (r.width, r.height))
                                        .unwrap_or((0, 0));
                                    // Calculate debugger tree height.
                                    // Layout is: 20% width. Height is full area minus borders?
                                    // In debugger.rs: chunks[0] is File Explorer.
                                    // We stored height in app_state.debugger_tree_height
                                    let tree_height = *app_state.debugger_tree_height.borrow();
                                    app_state
                                        .update_debugger_tree_scroll(tree_height.saturating_sub(2));
                                }
                                app_state::Focus::DebuggerSource => {
                                    if let Some(current) = app_state.source_selected_line {
                                        if current > 0 {
                                            app_state.source_selected_line = Some(current - 1);
                                            if current - 1 < app_state.source_scroll_offset {
                                                app_state.source_scroll_offset = current - 1;
                                            }
                                        }
                                    }
                                }
                                _ => {}
                            },
                            KeyCode::Down => match app_state.focus {
                                app_state::Focus::Tree => {
                                    if app_state.current_tab == app_state::Tab::Inspector {
                                        app_state.move_selection(1);
                                        let (cols, rows) = terminal
                                            .size()
                                            .map(|r| (r.width, r.height))
                                            .unwrap_or((0, 0));
                                        let tree_height = (rows.saturating_sub(3 + 10)) as usize; // Approx tree height
                                        let tree_width = (cols as f32 * 0.75) as usize;
                                        app_state.update_tree_scroll(tree_height.saturating_sub(2));
                                        app_state.ensure_horizontal_visibility(
                                            tree_width.saturating_sub(2),
                                        );

                                        // Request details
                                        if let Some(node) = app_state.get_selected_node() {
                                            if let Some(id) = AppState::get_node_id(node) {
                                                log::info!("UI: Requesting details for id: {}", id);
                                                let _ = tx_details_request.try_send(id);
                                            } else {
                                                log::warn!(
                                                    "UI: Selected node has no object_id or value_id"
                                                );
                                            }
                                        } else {
                                            log::warn!("UI: No node selected");
                                        }
                                    }
                                }
                                app_state::Focus::Logs => app_state.scroll_logs(1),
                                app_state::Focus::DebuggerFiles => {
                                    app_state.move_debugger_selection(1);
                                    let tree_height = *app_state.debugger_tree_height.borrow();
                                    app_state
                                        .update_debugger_tree_scroll(tree_height.saturating_sub(2));
                                }
                                app_state::Focus::DebuggerSource => {
                                    if let Some(current) = app_state.source_selected_line {
                                        if let Some(content) = &app_state.open_file_content {
                                            if current < content.len().saturating_sub(1) {
                                                app_state.source_selected_line = Some(current + 1);
                                                let inner_height = app_state
                                                    .debugger_source_area
                                                    .borrow()
                                                    .height
                                                    .saturating_sub(2)
                                                    as usize;
                                                if current + 1
                                                    >= app_state.source_scroll_offset + inner_height
                                                {
                                                    app_state.source_scroll_offset =
                                                        current + 1 - inner_height + 1;
                                                }
                                            }
                                        }
                                    }
                                }
                                _ => {}
                            },
                            KeyCode::Left => {
                                if app_state.focus == app_state::Focus::Tree
                                    && app_state.current_tab == app_state::Tab::Inspector
                                {
                                    if key.modifiers.contains(event::KeyModifiers::SHIFT) {
                                        app_state.scroll_tree_horizontal(-1);
                                    } else if !app_state.collapse_selected() {
                                        app_state.select_parent();
                                        let (cols, rows) = terminal
                                            .size()
                                            .map(|r| (r.width, r.height))
                                            .unwrap_or((0, 0));
                                        let tree_height = (rows.saturating_sub(3 + 10)) as usize;
                                        let tree_width = (cols as f32 * 0.75) as usize;
                                        app_state.update_tree_scroll(tree_height.saturating_sub(2));
                                        app_state.ensure_horizontal_visibility(
                                            tree_width.saturating_sub(2),
                                        );

                                        // Request details
                                        if let Some(node) = app_state.get_selected_node() {
                                            if let Some(id) = AppState::get_node_id(node) {
                                                log::info!("UI: Requesting details for id: {}", id);
                                                let _ = tx_details_request.try_send(id);
                                            }
                                        }
                                    }
                                } else if app_state.focus == app_state::Focus::DebuggerFiles {
                                    app_state.toggle_debugger_expand();
                                }
                            }
                            KeyCode::Right => {
                                if app_state.focus == app_state::Focus::Tree
                                    && app_state.current_tab == app_state::Tab::Inspector
                                {
                                    if key.modifiers.contains(event::KeyModifiers::SHIFT) {
                                        app_state.scroll_tree_horizontal(1);
                                    } else if !app_state.expand_selected() {
                                        app_state.select_first_child();
                                        let (cols, rows) = terminal
                                            .size()
                                            .map(|r| (r.width, r.height))
                                            .unwrap_or((0, 0));
                                        let tree_height = (rows.saturating_sub(3 + 10)) as usize;
                                        let tree_width = (cols as f32 * 0.75) as usize;
                                        app_state.update_tree_scroll(tree_height.saturating_sub(2));
                                        app_state.ensure_horizontal_visibility(
                                            tree_width.saturating_sub(2),
                                        );

                                        // Request details
                                        if let Some(node) = app_state.get_selected_node() {
                                            if let Some(id) = AppState::get_node_id(node) {
                                                log::info!("UI: Requesting details for id: {}", id);
                                                let _ = tx_details_request.try_send(id);
                                            }
                                        }
                                    }
                                } else if app_state.focus == app_state::Focus::DebuggerFiles {
                                    app_state.toggle_debugger_expand();
                                }
                            }
                            KeyCode::Enter | KeyCode::Char(' ') => match app_state.focus {
                                app_state::Focus::IsolateSelection => {
                                    if let Some(isolate) = app_state
                                        .available_isolates
                                        .get(app_state.selected_isolate_index)
                                    {
                                        let id = &isolate.id;
                                        log::info!("Selecting isolate: {}", id);
                                        let _ = tx_selected_isolate.try_send(id.clone());
                                        app_state.show_isolate_selection = false;
                                        app_state.focus = app_state::Focus::Tree;
                                    }
                                }
                                app_state::Focus::DebuggerFiles => {
                                    app_state.activate_selected_debugger_node();
                                }
                                _ => {}
                            },
                            KeyCode::Char('b') => {
                                if app_state.focus == app_state::Focus::DebuggerSource {
                                    app_state.toggle_breakpoint();
                                }
                            }
                            KeyCode::PageUp => {
                                if app_state.focus == app_state::Focus::Logs {
                                    app_state.scroll_logs(-10);
                                } else if app_state.focus == app_state::Focus::DebuggerSource {
                                    if app_state.source_scroll_offset > 10 {
                                        app_state.source_scroll_offset -= 10;
                                    } else {
                                        app_state.source_scroll_offset = 0;
                                    }
                                }
                            }
                            KeyCode::PageDown => {
                                if app_state.focus == app_state::Focus::Logs {
                                    app_state.scroll_logs(10);
                                } else if app_state.focus == app_state::Focus::DebuggerSource {
                                    app_state.source_scroll_offset += 10;
                                }
                            }
                            KeyCode::F(5) => {
                                let _ = tx_refresh.try_send(());
                            }
                            _ => {}
                        }
                    }
                }
                Event::Mouse(mouse) => {
                    if !app_state.show_isolate_selection {
                        match mouse.kind {
                            event::MouseEventKind::Down(event::MouseButton::Left) => {
                                // App Bar Click Handling
                                if mouse.row < 3 {
                                    // Button width is 20
                                    let button_index = (mouse.column as usize) / 20;
                                    match button_index {
                                        0 => app_state.current_tab = app_state::Tab::Inspector,
                                        1 => app_state.current_tab = app_state::Tab::Debugger,
                                        2 => {
                                            // Hot Reload
                                            if let Some(tx) = &app_state.tx_flutter_command {
                                                let _ = tx.send("r".to_string()).await;
                                            }
                                        }
                                        3 => {
                                            // Hot Restart
                                            if let Some(tx) = &app_state.tx_flutter_command {
                                                let _ = tx.send("R".to_string()).await;
                                            }
                                        }
                                        4 => {
                                            // Auto Hot Reload Toggle
                                            app_state.auto_reload = !app_state.auto_reload;
                                            log::info!(
                                                "Auto Hot Reload: {}",
                                                if app_state.auto_reload { "ON" } else { "OFF" }
                                            );
                                        }
                                        5 => {
                                            // Refresh Isolates
                                            let _ = tx_refresh.try_send(());
                                        }
                                        6 => {
                                            // Logs Toggle
                                            app_state.show_logs = !app_state.show_logs;
                                        }
                                        7 => {
                                            // Quit
                                            if let Some(tx) = &app_state.tx_flutter_command {
                                                let _ = tx.send("q".to_string()).await;
                                            }
                                            break;
                                        }
                                        _ => {}
                                    }
                                } else {
                                    // Tree Interaction
                                    let x = mouse.column;
                                    let y = mouse.row;

                                    // Inspector Tree
                                    if app_state.current_tab == app_state::Tab::Inspector {
                                        let inspector_area =
                                            *app_state.inspector_tree_area.borrow();
                                        if x >= inspector_area.x
                                            && x < inspector_area.x + inspector_area.width
                                            && y >= inspector_area.y
                                            && y < inspector_area.y + inspector_area.height
                                        {
                                            app_state.focus = app_state::Focus::Tree;
                                            let relative_y = (y - inspector_area.y) as usize;
                                            let index = relative_y + app_state.tree_scroll_offset;

                                            let count = *app_state.inspector_visible_count.borrow();
                                            if index < count {
                                                if index == app_state.selected_index {
                                                    app_state.toggle_expand();
                                                } else {
                                                    app_state.selected_index = index;
                                                    // Request details
                                                    if let Some(node) =
                                                        app_state.get_selected_node()
                                                    {
                                                        if let Some(id) =
                                                            AppState::get_node_id(node)
                                                        {
                                                            log::info!(
                                                                "UI: Requesting details for id: {}",
                                                                id
                                                            );
                                                            let _ = tx_details_request.try_send(id);
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }

                                    // Debugger Tree
                                    if app_state.current_tab == app_state::Tab::Debugger {
                                        let debugger_area = *app_state.debugger_tree_area.borrow();
                                        if x >= debugger_area.x
                                            && x < debugger_area.x + debugger_area.width
                                            && y >= debugger_area.y
                                            && y < debugger_area.y + debugger_area.height
                                        {
                                            app_state.focus = app_state::Focus::DebuggerFiles;
                                            let relative_y = (y - debugger_area.y) as usize;
                                            let index =
                                                relative_y + app_state.debugger_tree_scroll_offset;

                                            let count = *app_state.debugger_visible_count.borrow();
                                            if index < count {
                                                if index == app_state.debugger_selected_index {
                                                    app_state.activate_selected_debugger_node();
                                                } else {
                                                    app_state.debugger_selected_index = index;
                                                }
                                            }
                                        }
                                    }

                                    if app_state.current_tab == app_state::Tab::Debugger {
                                        let source_area = *app_state.debugger_source_area.borrow();
                                        if x >= source_area.x
                                            && x < source_area.x + source_area.width
                                            && y >= source_area.y
                                            && y < source_area.y + source_area.height
                                        {
                                            app_state.focus = app_state::Focus::DebuggerSource;
                                            // Calculate clicked line
                                            let relative_y =
                                                y.saturating_sub(source_area.y) as usize;
                                            let line_index =
                                                app_state.source_scroll_offset + relative_y;
                                            app_state.source_selected_line = Some(line_index);
                                        }
                                    }
                                }
                            }
                            event::MouseEventKind::ScrollDown => {
                                let x = mouse.column;
                                let y = mouse.row;

                                // Inspector
                                let inspector_area = *app_state.inspector_tree_area.borrow();
                                if x >= inspector_area.x
                                    && x < inspector_area.x + inspector_area.width
                                    && y >= inspector_area.y
                                    && y < inspector_area.y + inspector_area.height
                                {
                                    app_state.scroll_tree(1);
                                }

                                // Debugger
                                let debugger_area = *app_state.debugger_tree_area.borrow();
                                if x >= debugger_area.x
                                    && x < debugger_area.x + debugger_area.width
                                    && y >= debugger_area.y
                                    && y < debugger_area.y + debugger_area.height
                                {
                                    app_state.move_debugger_selection(1);
                                }

                                // Logs
                                let (_, rows) = terminal
                                    .size()
                                    .map(|r| (r.width, r.height))
                                    .unwrap_or((0, 0));
                                if app_state.show_logs && y >= rows.saturating_sub(10) {
                                    app_state.scroll_logs(1);
                                }

                                // Debugger Source
                                let source_area = *app_state.debugger_source_area.borrow();
                                if x >= source_area.x
                                    && x < source_area.x + source_area.width
                                    && y >= source_area.y
                                    && y < source_area.y + source_area.height
                                {
                                    app_state.source_scroll_offset += 1;
                                }
                            }
                            event::MouseEventKind::ScrollUp => {
                                let x = mouse.column;
                                let y = mouse.row;

                                // Inspector
                                let inspector_area = *app_state.inspector_tree_area.borrow();
                                if x >= inspector_area.x
                                    && x < inspector_area.x + inspector_area.width
                                    && y >= inspector_area.y
                                    && y < inspector_area.y + inspector_area.height
                                {
                                    app_state.scroll_tree(-1);
                                }

                                // Debugger
                                let debugger_area = *app_state.debugger_tree_area.borrow();
                                if x >= debugger_area.x
                                    && x < debugger_area.x + debugger_area.width
                                    && y >= debugger_area.y
                                    && y < debugger_area.y + debugger_area.height
                                {
                                    app_state.move_debugger_selection(-1);
                                }

                                // Logs
                                let (_, rows) = terminal
                                    .size()
                                    .map(|r| (r.width, r.height))
                                    .unwrap_or((0, 0));
                                if app_state.show_logs && y >= rows.saturating_sub(10) {
                                    app_state.scroll_logs(-1);
                                }

                                // Debugger Source
                                let source_area = *app_state.debugger_source_area.borrow();
                                if x >= source_area.x
                                    && x < source_area.x + source_area.width
                                    && y >= source_area.y
                                    && y < source_area.y + source_area.height
                                {
                                    if app_state.source_scroll_offset > 0 {
                                        app_state.source_scroll_offset -= 1;
                                    }
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
