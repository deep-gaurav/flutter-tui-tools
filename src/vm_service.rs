use anyhow::{Context, Result};
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::net::TcpStream;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};

pub struct VmServiceClient {
    ws_stream: WebSocketStream<MaybeTlsStream<TcpStream>>,
    request_id: u64,
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
    // Add other fields as needed
}

#[derive(Debug, Serialize, Deserialize)]
pub struct VM {
    pub isolates: Vec<IsolateRef>,
}

#[derive(Debug, Serialize, Deserialize)]
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
    pub async fn connect(uri: &str) -> Result<Self> {
        let (ws_stream, _) = connect_async(uri)
            .await
            .context("Failed to connect to WebSocket")?;
        Ok(Self {
            ws_stream,
            request_id: 0,
        })
    }

    async fn send_request(&mut self, method: &str, params: Value) -> Result<Value> {
        self.request_id += 1;
        let request = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
            "id": self.request_id,
        });

        self.ws_stream
            .send(tokio_tungstenite::tungstenite::Message::Text(
                request.to_string(),
            ))
            .await?;

        // Simple read loop for now, assuming response comes next.
        // In a real app, we'd need a proper response handler loop.
        while let Some(msg) = self.ws_stream.next().await {
            let msg = msg?;
            if let tokio_tungstenite::tungstenite::Message::Text(text) = msg {
                let mut deserializer = serde_json::Deserializer::from_str(&text);
                deserializer.disable_recursion_limit();
                let response: Value = Value::deserialize(&mut deserializer)?;
                if let Some(id) = response.get("id") {
                    if id.as_u64() == Some(self.request_id) {
                        if let Some(result) = response.get("result") {
                            return Ok(result.clone());
                        }
                        if let Some(error) = response.get("error") {
                            return Err(anyhow::anyhow!("RPC Error: {:?}", error));
                        }
                    }
                }
            }
        }

        Err(anyhow::anyhow!("Connection closed"))
    }

    pub async fn get_vm(&mut self) -> Result<VM> {
        let result = self.send_request("getVM", json!({})).await?;
        let vm: VM = serde_json::from_value(result)?;
        Ok(vm)
    }

    pub async fn get_isolate(&mut self, isolate_id: &str) -> Result<Isolate> {
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
        &mut self,
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

        log::debug!(
            "getRootWidgetSummaryTree response: {}",
            serde_json::to_string_pretty(&result).unwrap()
        );

        let node_json = if result.get("type").and_then(|t| t.as_str()) == Some("_extensionType") {
            result.get("result").unwrap_or(&result)
        } else {
            &result
        };

        let node: RemoteDiagnosticsNode = serde_json::from_value(node_json.clone())?;
        Ok(node)
    }
}
