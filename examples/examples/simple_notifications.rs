//! Simple example showing server-initiated notifications using ConnectionManager.
//!
//! Run with: cargo run --example simple_notifications
//! Test with: websocat ws://127.0.0.1:8888

use std::sync::Arc;
use std::time::Duration;

use jsonrpsee::core::{RpcResult, async_trait};
use jsonrpsee::proc_macros::rpc;
use jsonrpsee::server::{ConnectionManager, Notification, Server, ServerConfigBuilder};
use serde_json::json;

#[rpc(server, namespace = "demo")]
pub trait SimpleApi {
	#[method(name = "broadcast")]
	async fn broadcast(&self, message: String) -> RpcResult<String>;
}

pub struct SimpleApiImpl {
	connection_manager: Arc<ConnectionManager>,
}

#[async_trait]
impl SimpleApiServer for SimpleApiImpl {
	async fn broadcast(&self, message: String) -> RpcResult<String> {
		let notification = Notification::new(
			"broadcast",
			json!({
				"message": message,
				"timestamp": std::time::SystemTime::now()
					.duration_since(std::time::UNIX_EPOCH)
					.unwrap()
					.as_secs()
			}),
		);

		match self.connection_manager.broadcast_notification(notification).await {
			Ok(count) => Ok(format!("Sent to {} connections", count)),
			Err(e) => Ok(format!("Error: {}", e)),
		}
	}
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
	tracing_subscriber::fmt::init();

	// Create connection manager
	let connection_manager = Arc::new(ConnectionManager::new());

	// Configure server with connection manager
	let config = ServerConfigBuilder::default().set_connection_manager(connection_manager.clone()).build();

	let server = Server::builder().set_config(config).build("127.0.0.1:8888").await?;

	// Create RPC module
	let api = SimpleApiImpl { connection_manager: connection_manager.clone() };
	let module = api.into_rpc();

	// Start heartbeat task
	tokio::spawn({
		let conn_mgr = connection_manager.clone();
		async move {
			let mut counter = 0;
			loop {
				tokio::time::sleep(Duration::from_secs(5)).await;
				counter += 1;

				let notification = Notification::new(
					"heartbeat",
					json!({
						"counter": counter,
						"message": format!("Heartbeat #{}", counter)
					}),
				);

				if let Ok(count) = conn_mgr.broadcast_notification(notification).await {
					if count > 0 {
						println!("💓 Sent heartbeat to {} connections", count);
					}
				}
			}
		}
	});

	println!("🚀 Server started at ws://127.0.0.1:8888");
	println!("📡 Heartbeat every 5 seconds");
	println!("🔗 Connect with: websocat ws://127.0.0.1:8888");
	println!(
		"📨 Test broadcast: echo '{{\"jsonrpc\":\"2.0\",\"method\":\"demo.broadcast\",\"params\":[\"Hello!\"],\"id\":1}}' | websocat ws://127.0.0.1:8888"
	);

	let handle = server.start(module);
	handle.stopped().await;

	Ok(())
}
