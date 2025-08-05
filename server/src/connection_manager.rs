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

//! Connection manager for handling active WebSocket connections and sending notifications.

use std::collections::HashMap;
use std::sync::Arc;

use jsonrpsee_core::server::{ConnectionId, MethodSink};
use serde::Serialize;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

use crate::LOG_TARGET;

/// Error types for notification operations.
#[derive(Debug, thiserror::Error)]
pub enum NotificationError {
	/// The specified connection ID was not found in the active connections.
	#[error("Connection {0:?} not found")]
	ConnectionNotFound(ConnectionId),
	/// The connection was closed before the notification could be sent.
	#[error("Connection closed")]
	ConnectionClosed,
	/// Failed to serialize the notification message to JSON.
	#[error("Serialization error: {0}")]
	SerializationError(#[from] serde_json::Error),
	/// Failed to send the notification through the connection sink.
	#[error("Send error")]
	SendError,
}

/// A JSON-RPC 2.0 notification message.
#[derive(Debug, Clone, Serialize)]
pub struct Notification<T> {
	jsonrpc: String,
	method: String,
	params: T,
}

impl<T> Notification<T> {
	/// Create a new notification with the given method and parameters.
	pub fn new(method: impl Into<String>, params: T) -> Self {
		Self { jsonrpc: "2.0".to_string(), method: method.into(), params }
	}
}

/// Statistics about active connections.
#[derive(Debug, Clone)]
pub struct ConnectionStats {
	/// The total number of active connections currently managed.
	pub total_connections: usize,
	/// A list of all active connection IDs.
	pub active_connections: Vec<ConnectionId>,
}

/// Manages active WebSocket connections and provides notification functionality.
#[derive(Debug, Clone)]
pub struct ConnectionManager {
	connections: Arc<RwLock<HashMap<ConnectionId, MethodSink>>>,
}

impl ConnectionManager {
	/// Create a new connection manager.
	pub fn new() -> Self {
		Self { connections: Arc::new(RwLock::new(HashMap::new())) }
	}

	/// Register a new connection with its associated method sink.
	pub async fn register_connection(&self, conn_id: ConnectionId, sink: MethodSink) {
		self.connections.write().await.insert(conn_id, sink);
		info!(target: LOG_TARGET, "Connection {} registered for notifications", conn_id.0);
	}

	/// Unregister a connection when it's closed.
	pub async fn unregister_connection(&self, conn_id: ConnectionId) {
		if self.connections.write().await.remove(&conn_id).is_some() {
			info!(target: LOG_TARGET, "Connection {} unregistered from notifications", conn_id.0);
		}
	}

	/// Send a notification to a specific connection.
	pub async fn send_notification<T: Serialize>(
		&self,
		conn_id: ConnectionId,
		notification: Notification<T>,
	) -> Result<(), NotificationError> {
		let connections = self.connections.read().await;

		if let Some(sink) = connections.get(&conn_id) {
			let json = serde_json::value::to_raw_value(&notification)?;

			sink.send(json).await.map_err(|_| {
				warn!(target: LOG_TARGET, "Failed to send notification to connection {}", conn_id.0);
				NotificationError::ConnectionClosed
			})?;

			debug!(target: LOG_TARGET, "Notification sent to connection {}: {}", conn_id.0, notification.method);
			Ok(())
		} else {
			warn!(target: LOG_TARGET, "Connection {} not found for notification", conn_id.0);
			Err(NotificationError::ConnectionNotFound(conn_id))
		}
	}

	/// Send a notification to multiple specific connections.
	pub async fn send_to_connections<T: Serialize>(
		&self,
		conn_ids: &[ConnectionId],
		notification: Notification<T>,
	) -> Result<Vec<Result<(), NotificationError>>, NotificationError> {
		let json = serde_json::value::to_raw_value(&notification)?;
		let connections = self.connections.read().await;

		let mut results = Vec::new();

		for &conn_id in conn_ids {
			let result = if let Some(sink) = connections.get(&conn_id) {
				sink.send(json.clone()).await.map_err(|_| {
					warn!(target: LOG_TARGET, "Failed to send notification to connection {}", conn_id.0);
					NotificationError::ConnectionClosed
				})
			} else {
				warn!(target: LOG_TARGET, "Connection {} not found for notification", conn_id.0);
				Err(NotificationError::ConnectionNotFound(conn_id))
			};
			results.push(result);
		}

		debug!(target: LOG_TARGET, "Notification sent to {} connections: {}", conn_ids.len(), notification.method);
		Ok(results)
	}

	/// Broadcast a notification to all active connections.
	pub async fn broadcast_notification<T: Serialize>(
		&self,
		notification: Notification<T>,
	) -> Result<usize, NotificationError> {
		let json = serde_json::value::to_raw_value(&notification)?;
		let connections = self.connections.read().await;

		let mut sent_count = 0;
		let mut failed_connections = Vec::new();

		for (&conn_id, sink) in connections.iter() {
			match sink.send(json.clone()).await {
				Ok(()) => {
					sent_count += 1;
					debug!(target: LOG_TARGET, "Broadcast notification sent to connection {}", conn_id.0);
				}
				Err(_) => {
					failed_connections.push(conn_id);
					warn!(target: LOG_TARGET, "Failed to send broadcast notification to connection {}", conn_id.0);
				}
			}
		}

		info!(target: LOG_TARGET, "Broadcast notification '{}' sent to {}/{} connections", 
              notification.method, sent_count, connections.len());

		// Clean up failed connections in background
		if !failed_connections.is_empty() {
			let manager = self.clone();
			tokio::spawn(async move {
				for conn_id in failed_connections {
					manager.unregister_connection(conn_id).await;
				}
			});
		}

		Ok(sent_count)
	}

	/// Broadcast a notification to connections that match a filter predicate.
	pub async fn broadcast_filtered<T, F>(
		&self,
		notification: Notification<T>,
		filter: F,
	) -> Result<usize, NotificationError>
	where
		T: Serialize,
		F: Fn(ConnectionId) -> bool,
	{
		let json = serde_json::value::to_raw_value(&notification)?;
		let connections = self.connections.read().await;

		let mut sent_count = 0;
		let mut failed_connections = Vec::new();

		for (&conn_id, sink) in connections.iter() {
			if filter(conn_id) {
				match sink.send(json.clone()).await {
					Ok(()) => {
						sent_count += 1;
						debug!(target: LOG_TARGET, "Filtered notification sent to connection {}", conn_id.0);
					}
					Err(_) => {
						failed_connections.push(conn_id);
						warn!(target: LOG_TARGET, "Failed to send filtered notification to connection {}", conn_id.0);
					}
				}
			}
		}

		info!(target: LOG_TARGET, "Filtered notification '{}' sent to {} connections", 
              notification.method, sent_count);

		// Clean up failed connections in background
		if !failed_connections.is_empty() {
			let manager = self.clone();
			tokio::spawn(async move {
				for conn_id in failed_connections {
					manager.unregister_connection(conn_id).await;
				}
			});
		}

		Ok(sent_count)
	}

	/// Get statistics about active connections.
	pub async fn get_stats(&self) -> ConnectionStats {
		let connections = self.connections.read().await;
		ConnectionStats {
			total_connections: connections.len(),
			active_connections: connections.keys().copied().collect(),
		}
	}

	/// Get the number of active connections.
	pub async fn connection_count(&self) -> usize {
		self.connections.read().await.len()
	}

	/// Check if a specific connection is active.
	pub async fn is_connection_active(&self, conn_id: ConnectionId) -> bool {
		self.connections.read().await.contains_key(&conn_id)
	}
}

impl Default for ConnectionManager {
	fn default() -> Self {
		Self::new()
	}
}

/// Macro for sending notifications conveniently.
#[macro_export]
macro_rules! notify {
	// Send to specific connection
	($manager:expr, $conn_id:expr, $method:expr, $params:expr) => {
		$manager.send_notification($conn_id, $crate::connection_manager::Notification::new($method, $params)).await
	};

	// Broadcast to all connections
	(broadcast $manager:expr, $method:expr, $params:expr) => {
		$manager.broadcast_notification($crate::connection_manager::Notification::new($method, $params)).await
	};

	// Send to multiple connections
	(multi $manager:expr, $conn_ids:expr, $method:expr, $params:expr) => {
		$manager.send_to_connections($conn_ids, $crate::connection_manager::Notification::new($method, $params)).await
	};

	// Send with filter
	(filter $manager:expr, $filter:expr, $method:expr, $params:expr) => {
		$manager.broadcast_filtered($crate::connection_manager::Notification::new($method, $params), $filter).await
	};
}
