//! `RealNode`: the `FiberRpc` implementation that talks to an actual FNN
//! JSON-RPC endpoint over HTTP.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::time::Duration;

use crate::error::NodeError;
use crate::payments::{ListPaymentsParams, ListPaymentsResult, PaymentInfo, SendPaymentParams};
use crate::rpc_types::{Channel, ListChannelsParams, ListChannelsResult, NodeInfo};
use crate::FiberRpc;

/// A Fiber node reached over JSON-RPC 2.0.
///
/// FNN wraps request params as a single-object array (`"params": [{...}]`),
/// or an empty array for parameterless methods — `call` handles both.
#[derive(Debug, Clone)]
pub struct RealNode {
    url: String,
    client: reqwest::Client,
}

#[derive(Serialize)]
struct RpcRequest<'a> {
    jsonrpc: &'static str,
    id: u64,
    method: &'a str,
    params: serde_json::Value,
}

#[derive(Deserialize)]
struct RpcEnvelope {
    #[serde(default)]
    result: Option<serde_json::Value>,
    #[serde(default)]
    error: Option<RpcErrorBody>,
}

#[derive(Deserialize)]
struct RpcErrorBody {
    code: i64,
    message: String,
}

impl RealNode {
    /// Connect to the node RPC at `url` (e.g. `http://127.0.0.1:8227`).
    /// Requests time out after 15 s so a hung node degrades the poller to
    /// stale-data mode instead of wedging the control loop.
    pub fn new(url: impl Into<String>) -> Result<Self, NodeError> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .build()?;
        Ok(Self { url: url.into(), client })
    }

    async fn call<R: DeserializeOwned>(
        &self,
        method: &'static str,
        params: serde_json::Value,
    ) -> Result<R, NodeError> {
        let req = RpcRequest { jsonrpc: "2.0", id: 1, method, params };
        let envelope: RpcEnvelope = self
            .client
            .post(&self.url)
            .json(&req)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        if let Some(err) = envelope.error {
            return Err(NodeError::Rpc { code: err.code, message: err.message });
        }
        let result = envelope.result.unwrap_or(serde_json::Value::Null);
        serde_json::from_value(result).map_err(|source| NodeError::Decode { method, source })
    }
}

#[async_trait::async_trait]
impl FiberRpc for RealNode {
    async fn node_info(&self) -> Result<NodeInfo, NodeError> {
        self.call("node_info", serde_json::json!([])).await
    }

    async fn list_channels(&self, params: ListChannelsParams) -> Result<Vec<Channel>, NodeError> {
        let result: ListChannelsResult = self
            .call("list_channels", serde_json::json!([params]))
            .await?;
        Ok(result.channels)
    }

    async fn send_payment(&self, params: SendPaymentParams) -> Result<PaymentInfo, NodeError> {
        self.call("send_payment", serde_json::json!([params])).await
    }

    async fn get_payment(&self, payment_hash: &str) -> Result<PaymentInfo, NodeError> {
        self.call(
            "get_payment",
            serde_json::json!([{ "payment_hash": payment_hash }]),
        )
        .await
    }

    async fn list_payments(
        &self,
        params: ListPaymentsParams,
    ) -> Result<ListPaymentsResult, NodeError> {
        self.call("list_payments", serde_json::json!([params])).await
    }
}
