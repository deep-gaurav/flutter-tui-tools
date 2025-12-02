use anyhow::{Context, Result};
use regex::Regex;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;

pub struct FlutterDaemon {
    uri_sender: mpsc::Sender<String>,
}

impl FlutterDaemon {
    pub fn new(uri_sender: mpsc::Sender<String>) -> Self {
        Self { uri_sender }
    }

    pub async fn run(&self, app_dir: &str, device_id: Option<&str>) -> Result<()> {
        let mut cmd = Command::new("fvm");
        cmd.arg("flutter")
            .arg("attach")
            // .arg("--machine")
            .arg("--verbose")
            .current_dir(app_dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        if let Some(id) = device_id {
            cmd.arg("-d").arg(id);
        }

        let mut child = cmd.spawn().context("Failed to spawn fvm flutter attach")?;

        let stdout = child.stdout.take().context("Failed to open stdout")?;
        let stderr = child.stderr.take().context("Failed to open stderr")?;

        // Spawn stderr reader
        tokio::spawn(async move {
            let mut reader = BufReader::new(stderr);
            let mut line = String::new();
            loop {
                line.clear();
                match reader.read_line(&mut line).await {
                    Ok(0) => break, // EOF
                    Ok(_) => {
                        let trimmed = line.trim();
                        if !trimmed.is_empty() {
                            log::error!("Flutter Error: {}", trimmed);
                        }
                    }
                    Err(e) => {
                        log::error!("Error reading stderr: {}", e);
                        break;
                    }
                }
            }
        });

        let mut reader = BufReader::new(stdout);
        let mut line = String::new();

        // Regex to capture the URI.
        // Matches "available at: http://..."
        let re = Regex::new(r"available at: (http://[\d\.:]+/[^/]+/?)").unwrap();

        loop {
            line.clear();
            let bytes_read = reader.read_line(&mut line).await?;
            if bytes_read == 0 {
                break;
            }

            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }

            log::info!("Flutter Output: {}", trimmed);

            if let Some(caps) = re.captures(trimmed) {
                if let Some(uri_match) = caps.get(1) {
                    let uri = uri_match.as_str().to_string();
                    // Convert http to ws if necessary, but tungstenite might handle http uri by replacing scheme?
                    // Usually VM service accepts ws://.
                    // The output is http://.../
                    // We should convert it to ws://
                    let ws_uri = uri.replace("http://", "ws://");
                    let _ = self.uri_sender.send(ws_uri).await;
                }
            }
        }

        Ok(())
    }
}
