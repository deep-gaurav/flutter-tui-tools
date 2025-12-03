use anyhow::{Context, Result};
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use tokio::net::TcpStream;
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VmEvent {
    pub stream_id: String,
    pub event_kind: String,
    pub isolate_id: Option<String>,
    pub timestamp: i64,
    pub data: Value,
}

#[derive(Clone)]
pub struct VmServiceClient {
    tx_request: mpsc::Sender<RequestMessage>,
    // We might want to support multiple event listeners in the future,
    // but for now a single receiver is enough.
    // Actually, we'll let the user take the receiver.
}

struct RequestMessage {
    method: String,
    params: Value,
    tx_response: oneshot::Sender<Result<Value>>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RemoteDiagnosticsNode {
    pub description: Option<String>,
    #[serde(rename = "type")]
    pub node_type: Option<String>,
    pub name: Option<String>,
    pub style: Option<String>,
    #[serde(rename = "hasChildren")]
    pub has_children: Option<bool>,
    pub children: Option<Vec<RemoteDiagnosticsNode>>,
    #[serde(rename = "widgetRuntimeType")]
    pub widget_runtime_type: Option<String>,
    #[serde(rename = "objectId")]
    pub object_id: Option<String>,
    #[serde(rename = "valueId")]
    pub value_id: Option<String>,
    pub properties: Option<Vec<RemoteDiagnosticsNode>>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct VM {
    pub isolates: Vec<IsolateRef>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct IsolateRef {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Isolate {
    pub id: String,
    pub name: String,
    #[serde(rename = "extensionRPCs")]
    pub extension_rpcs: Option<Vec<String>>,
}

impl VmServiceClient {
    pub async fn connect(uri: &str) -> Result<(Self, mpsc::Receiver<VmEvent>)> {
        let (ws_stream, _) = connect_async(uri)
            .await
            .context("Failed to connect to WebSocket")?;

        let (tx_request, rx_request) = mpsc::channel(32);
        let (tx_event, rx_event) = mpsc::channel(100);

        tokio::spawn(async move {
            if let Err(e) = Self::driver_loop(ws_stream, rx_request, tx_event).await {
                log::error!("VM Service Driver Error: {}", e);
            }
        });

        Ok((Self { tx_request }, rx_event))
    }

    async fn driver_loop(
        mut ws_stream: WebSocketStream<MaybeTlsStream<TcpStream>>,
        mut rx_request: mpsc::Receiver<RequestMessage>,
        tx_event: mpsc::Sender<VmEvent>,
    ) -> Result<()> {
        let mut request_id = 0u64;
        let mut pending_requests: HashMap<u64, oneshot::Sender<Result<Value>>> = HashMap::new();

        loop {
            tokio::select! {
                Some(msg) = rx_request.recv() => {
                    request_id += 1;
                    let request_json = json!({
                        "jsonrpc": "2.0",
                        "method": msg.method,
                        "params": msg.params,
                        "id": request_id,
                    });

                    pending_requests.insert(request_id, msg.tx_response);

                    if let Err(e) = ws_stream.send(tokio_tungstenite::tungstenite::Message::Text(request_json.to_string())).await {
                        log::error!("Failed to send request: {}", e);
                        // We should probably remove the pending request and error it out
                        if let Some(tx) = pending_requests.remove(&request_id) {
                            let _ = tx.send(Err(anyhow::anyhow!("Failed to send request: {}", e)));
                        }
                    }
                }
                Some(msg) = ws_stream.next() => {
                    match msg {
                        Ok(tokio_tungstenite::tungstenite::Message::Text(text)) => {
                             if let Ok(response) = serde_json::from_str::<Value>(&text) {
                                // Check if it's a response or event
                                if let Some(id) = response.get("id").and_then(|id| id.as_u64()) {
                                    // It's a response
                                    if let Some(tx) = pending_requests.remove(&id) {
                                        if let Some(result) = response.get("result") {
                                            let _ = tx.send(Ok(result.clone()));
                                        } else if let Some(error) = response.get("error") {
                                            let _ = tx.send(Err(anyhow::anyhow!("RPC Error: {:?}", error)));
                                        } else {
                                             let _ = tx.send(Ok(response.clone())); // Fallback
                                        }
                                    }
                                } else if let Some(method) = response.get("method").and_then(|s| s.as_str()) {
                                    if method == "streamNotify" {
                                        // It's an event
                                        if let Some(params) = response.get("params") {
                                            let stream_id = params.get("streamId").and_then(|s| s.as_str()).unwrap_or("").to_string();
                                            let event_kind = params.get("event").and_then(|e| e.get("kind")).and_then(|s| s.as_str()).unwrap_or("").to_string();
                                            let isolate_id = params.get("event").and_then(|e| e.get("isolate")).and_then(|i| i.get("id")).and_then(|s| s.as_str()).map(|s| s.to_string());
                                            let timestamp = params.get("event").and_then(|e| e.get("timestamp")).and_then(|t| t.as_i64()).unwrap_or(0);
                                            let data = params.get("event").cloned().unwrap_or(Value::Null);

                                            let event = VmEvent {
                                                stream_id,
                                                event_kind,
                                                isolate_id,
                                                timestamp,
                                                data,
                                            };
                                            let _ = tx_event.send(event).await;
                                        }
                                    }
                                }
                            }
                        }
                        Ok(tokio_tungstenite::tungstenite::Message::Close(_)) => break,
                        Err(e) => {
                            log::error!("WebSocket error: {}", e);
                            break;
                        }
                        _ => {}
                    }
                }
                else => break,
            }
        }
        Ok(())
    }

    async fn send_request(&self, method: &str, params: Value) -> Result<Value> {
        let (tx, rx) = oneshot::channel();
        let msg = RequestMessage {
            method: method.to_string(),
            params,
            tx_response: tx,
        };

        self.tx_request
            .send(msg)
            .await
            .context("Failed to send request to driver")?;

        rx.await.context("Failed to receive response from driver")?
    }

    pub async fn stream_listen(&self, stream_id: &str) -> Result<()> {
        self.send_request("streamListen", json!({ "streamId": stream_id }))
            .await?;
        Ok(())
    }

    pub async fn get_vm(&self) -> Result<VM> {
        let result = self.send_request("getVM", json!({})).await?;
        let vm: VM = serde_json::from_value(result)?;
        Ok(vm)
    }

    pub async fn get_isolate(&self, isolate_id: &str) -> Result<Isolate> {
        let result = self
            .send_request(
                "getIsolate",
                json!({
                    "isolateId": isolate_id
                }),
            )
            .await?;
        let isolate: Isolate = serde_json::from_value(result)?;
        Ok(isolate)
    }

    pub async fn get_root_widget_summary_tree(
        &self,
        group: &str,
        isolate_id: &str,
    ) -> Result<RemoteDiagnosticsNode> {
        let result = self
            .send_request(
                "ext.flutter.inspector.getRootWidgetSummaryTree",
                json!({
                    "isolateId": isolate_id,
                    "objectGroup": group
                }),
            )
            .await?;

        // log::info!(
        //     "getRootWidgetSummaryTree response: {}",
        //     serde_json::to_string_pretty(&result).unwrap()
        // );

        let node_json = if result.get("type").and_then(|t| t.as_str()) == Some("_extensionType") {
            result.get("result").unwrap_or(&result)
        } else {
            &result
        };

        let node: RemoteDiagnosticsNode = serde_json::from_value(node_json.clone())?;
        Ok(node)
    }

    pub async fn get_details_subtree(
        &self,
        isolate_id: &str,
        object_id: &str,
        subtree_depth: i32,
    ) -> Result<RemoteDiagnosticsNode> {
        let result = self
            .send_request(
                "ext.flutter.inspector.getDetailsSubtree",
                json!({
                    "isolateId": isolate_id,
                    "objectGroup": "tui_inspector",
                    "arg": object_id,
                    "subtreeDepth": subtree_depth
                }),
            )
            .await?;

        let node_json = if result.get("type").and_then(|t| t.as_str()) == Some("_extensionType") {
            result.get("result").unwrap_or(&result)
        } else {
            &result
        };

        let node: RemoteDiagnosticsNode = serde_json::from_value(node_json.clone())?;
        Ok(node)
    }

    pub async fn add_breakpoint(
        &self,
        isolate_id: &str,
        script_id: &str,
        line: usize,
    ) -> Result<Value> {
        self.send_request(
            "addBreakpoint",
            json!({
                "isolateId": isolate_id,
                "scriptId": script_id,
                "line": line
            }),
        )
        .await
    }

    pub async fn add_breakpoint_with_script_uri(
        &self,
        isolate_id: &str,
        script_uri: &str,
        line: usize,
    ) -> Result<Value> {
        self.send_request(
            "addBreakpointWithScriptUri",
            json!({
                "isolateId": isolate_id,
                "scriptUri": script_uri,
                "line": line
            }),
        )
        .await
    }

    pub async fn remove_breakpoint(&self, isolate_id: &str, breakpoint_id: &str) -> Result<Value> {
        self.send_request(
            "removeBreakpoint",
            json!({
                "isolateId": isolate_id,
                "breakpointId": breakpoint_id
            }),
        )
        .await
    }

    pub async fn resume(&self, isolate_id: &str, step: Option<&str>) -> Result<Value> {
        let mut params = json!({
            "isolateId": isolate_id
        });
        if let Some(s) = step {
            params
                .as_object_mut()
                .unwrap()
                .insert("step".to_string(), json!(s));
        }
        self.send_request("resume", params).await
    }

    pub async fn pause(&self, isolate_id: &str) -> Result<Value> {
        self.send_request(
            "pause",
            json!({
                "isolateId": isolate_id
            }),
        )
        .await
    }

    pub async fn get_stack(&self, isolate_id: &str) -> Result<Value> {
        self.send_request(
            "getStack",
            json!({
                "isolateId": isolate_id
            }),
        )
        .await
    }

    pub async fn get_object(&self, isolate_id: &str, object_id: &str) -> Result<Value> {
        self.send_request(
            "getObject",
            json!({
                "isolateId": isolate_id,
                "objectId": object_id
            }),
        )
        .await
    }
}
