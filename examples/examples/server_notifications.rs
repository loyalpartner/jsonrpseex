// Copyright 2019-2021 Parity Technologies (UK) Ltd.
//
// Permission is hereby granted, free of charge, to any
// person obtaining a copy of this software and associated
// documentation files (the "Software"), to deal in the
// Software without restriction, including without
// limitation the rights to use, copy, modify, merge,
// publish, distribute, sublicense, and/or sell copies of
// the Software, and to permit persons to whom the Software
// is furnished to do so, subject to the following
// conditions:
//
// The above copyright notice and this permission notice
// shall be included in all copies or substantial portions
// of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
// ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
// TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
// PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
// SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
// CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
// OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
// IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
// DEALINGS IN THE SOFTWARE.

//! Example showing server-initiated notifications using ConnectionManager.
//! This allows the server to send notifications to clients without them needing to subscribe first.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use jsonrpsee::core::{RpcResult, async_trait};
use jsonrpsee::proc_macros::rpc;
use jsonrpsee::server::{ConnectionManager, Notification, Server, ServerConfigBuilder};
use jsonrpsee::types::ErrorObjectOwned;
use jsonrpsee::ws_client::WsClientBuilder;
// Note: rpc_params and Extensions would be used in a full implementation
use serde_json::json;
use tokio::time::sleep;

// Define RPC trait with notification support
#[rpc(server, client, namespace = "notification_demo")]
pub trait NotificationApi {
	/// Send a message to a specific connection
	#[method(name = "send_to_connection")]
	async fn send_to_connection(&self, conn_id: u32, message: String) -> RpcResult<String>;

	/// Broadcast a message to all connections
	#[method(name = "broadcast_message")]
	async fn broadcast_message(&self, message: String) -> RpcResult<String>;

	/// Get connection statistics
	#[method(name = "get_connection_stats")]
	async fn get_connection_stats(&self) -> RpcResult<serde_json::Value>;

	/// Get current connection ID
	#[method(name = "get_my_connection_id")]
	async fn get_my_connection_id(&self) -> RpcResult<u32>;

	/// Trigger a notification in RPC method
	#[method(name = "process_data")]
	async fn process_data(&self, data: String) -> RpcResult<String>;
}

// Implementation holding ConnectionManager
pub struct NotificationApiImpl {
	connection_manager: Arc<ConnectionManager>,
}

impl NotificationApiImpl {
	pub fn new(connection_manager: Arc<ConnectionManager>) -> Self {
		Self { connection_manager }
	}
}

#[async_trait]
impl NotificationApiServer for NotificationApiImpl {
	async fn send_to_connection(&self, conn_id: u32, message: String) -> RpcResult<String> {
		let notification = Notification::new(
			"private_message",
			json!({
				"from": "server",
				"message": message,
				"timestamp": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()
			}),
		);

		match self.connection_manager.send_notification(conn_id.into(), notification).await {
			Ok(()) => Ok(format!("Message sent to connection {}", conn_id)),
			Err(e) => Err(ErrorObjectOwned::owned(404, format!("Failed to send message: {}", e), None::<String>)),
		}
	}

	async fn broadcast_message(&self, message: String) -> RpcResult<String> {
		let notification = Notification::new(
			"broadcast",
			json!({
				"from": "server",
				"message": message,
				"timestamp": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()
			}),
		);

		match self.connection_manager.broadcast_notification(notification).await {
			Ok(sent_count) => Ok(format!("Message broadcasted to {} connections", sent_count)),
			Err(e) => Err(ErrorObjectOwned::owned(500, format!("Failed to broadcast: {}", e), None::<String>)),
		}
	}

	async fn get_connection_stats(&self) -> RpcResult<serde_json::Value> {
		let stats = self.connection_manager.get_stats().await;
		Ok(json!({
			"total_connections": stats.total_connections,
			"active_connection_ids": stats.active_connections.iter().map(|id| id.0).collect::<Vec<_>>()
		}))
	}

	async fn get_my_connection_id(&self) -> RpcResult<u32> {
		// Note: In real implementation, you would need access to Extensions
		// This is a simplified version for demonstration
		Ok(0) // Placeholder - would need to be implemented with proper context
	}

	async fn process_data(&self, data: String) -> RpcResult<String> {
		// Simplified version - in real implementation you'd get the connection ID from context
		let current_conn_id = 0;

		// Process the data (simulate some work)
		let result = format!("Processed: {}", data);

		// Send notification to other connections about the processing
		let notification = Notification::new(
			"data_processed",
			json!({
				"processed_by_connection": current_conn_id,
				"original_data": data,
				"result": &result,
				"timestamp": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()
			}),
		);

		// Broadcast to other connections (exclude current one)
		let filter = |conn_id: jsonrpsee::server::ConnectionId| conn_id.0 != current_conn_id;

		match self.connection_manager.broadcast_filtered(notification, filter).await {
			Ok(sent_count) => {
				tracing::info!("Notified {} other connections about data processing", sent_count);
			}
			Err(e) => {
				tracing::warn!("Failed to send processing notification: {}", e);
			}
		}

		Ok(result)
	}
}

async fn run_server() -> anyhow::Result<SocketAddr> {
	// Create connection manager
	let connection_manager = Arc::new(ConnectionManager::new());

	// Configure server with connection manager
	let server_config = ServerConfigBuilder::default().set_connection_manager(connection_manager.clone()).build();

	let server = Server::builder().set_config(server_config).build("127.0.0.1:8888").await?;

	let addr = server.local_addr()?;

	// Create RPC implementation with connection manager
	let api_impl = NotificationApiImpl::new(connection_manager.clone());
	let module = api_impl.into_rpc();

	// Start background task for periodic notifications
	tokio::spawn({
		let conn_mgr = connection_manager.clone();
		async move {
			let mut interval = tokio::time::interval(Duration::from_secs(10));
			let mut counter = 0;

			loop {
				interval.tick().await;
				counter += 1;

				let notification = Notification::new(
					"heartbeat",
					json!({
						"counter": counter,
						"timestamp": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs(),
						"message": format!("Server heartbeat #{}", counter)
					}),
				);

				if let Ok(sent_count) = conn_mgr.broadcast_notification(notification).await {
					if sent_count > 0 {
						tracing::info!("Sent heartbeat to {} connections", sent_count);
					}
				}
			}
		}
	});

	// Start server
	let handle = server.start(module);
	tokio::spawn(handle.stopped());

	Ok(addr)
}

async fn run_client(url: &str, client_id: u32) -> anyhow::Result<()> {
	let client = WsClientBuilder::default().build(url).await?;

	// Note: In a real implementation, you would set up a proper subscription
	// to receive server notifications. This is simplified for demonstration.

	// Get connection ID
	if let Ok(conn_id) = client.get_my_connection_id().await {
		tracing::info!("Client {} got connection ID: {}", client_id, conn_id);
	}

	// Test various notification features
	sleep(Duration::from_secs(2)).await;

	// Broadcast a message
	if let Ok(result) = client.broadcast_message(format!("Hello from client {}", client_id)).await {
		tracing::info!("Client {} broadcast result: {}", client_id, result);
	}

	sleep(Duration::from_secs(1)).await;

	// Process some data (will trigger notifications to other clients)
	if let Ok(result) = client.process_data(format!("data_from_client_{}", client_id)).await {
		tracing::info!("Client {} process result: {}", client_id, result);
	}

	sleep(Duration::from_secs(1)).await;

	// Get connection stats
	if let Ok(stats) = client.get_connection_stats().await {
		tracing::info!("Client {} sees connection stats: {}", client_id, stats);
	}

	Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
	tracing_subscriber::FmtSubscriber::builder()
		.with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
		.try_init()
		.expect("setting default subscriber failed");

	// Start server
	let server_addr = run_server().await?;
	let url = format!("ws://{}", server_addr);
	tracing::info!("Server started at {}", url);

	// Wait for server to start
	sleep(Duration::from_secs(1)).await;

	// Start multiple clients
	let client_handles = (1..=3)
		.map(|client_id| {
			let url = url.clone();
			tokio::spawn(async move {
				if let Err(e) = run_client(&url, client_id).await {
					tracing::error!("Client {} error: {}", client_id, e);
				}
			})
		})
		.collect::<Vec<_>>();

	// Let clients run for a while
	sleep(Duration::from_secs(30)).await;

	// Wait for all clients to finish
	for handle in client_handles {
		let _ = handle.await;
	}

	tracing::info!("Example completed");
	Ok(())
}
