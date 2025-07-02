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

use jsonrpsee::{
	core::client::ClientT,
	core::traits::MessageEncryption,
	server::{RpcModule, ServerBuilder},
	ws_client::WsClientBuilder,
};

/// Simple XOR encryption for demonstration purposes.
/// DO NOT use this in production - it's only for demonstration!
#[derive(Clone, Debug)]
pub struct SimpleXorEncryption {
	key: Vec<u8>,
}

impl SimpleXorEncryption {
	pub fn new(key: Vec<u8>) -> Self {
		Self { key }
	}
}

impl MessageEncryption for SimpleXorEncryption {
	fn encrypt(&self, data: &str) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
		let data_bytes = data.as_bytes();
		let mut encrypted = Vec::with_capacity(data_bytes.len());

		for (i, byte) in data_bytes.iter().enumerate() {
			let key_byte = self.key[i % self.key.len()];
			encrypted.push(byte ^ key_byte);
		}

		// Encode as hex string for transmission
		Ok(hex::encode(encrypted))
	}

	fn decrypt(&self, data: &str) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
		// Decode from hex
		let encrypted = hex::decode(data)?;
		let mut decrypted = Vec::with_capacity(encrypted.len());

		for (i, byte) in encrypted.iter().enumerate() {
			let key_byte = self.key[i % self.key.len()];
			decrypted.push(byte ^ key_byte);
		}

		Ok(String::from_utf8(decrypted)?)
	}
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
	tracing_subscriber::FmtSubscriber::builder()
		.with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
		.try_init()
		.expect("setting default subscriber failed");

	// Create encryption instance
	let encryption_key = b"my_secret_key".to_vec();
	let server_encryption = SimpleXorEncryption::new(encryption_key.clone());

	let server = ServerBuilder::default().set_message_encryption(server_encryption).build("127.0.0.1:0").await?;

	let mut module = RpcModule::new(());
	module.register_method("echo", |params, _, _| -> Result<String, jsonrpsee::types::ErrorObjectOwned> {
		let data: String = params.one()?;
		Ok(data)
	})?;

	let addr = server.local_addr()?;
	let server_handle = server.start(module);

	println!("Server started at {}", addr);

	// Create client with the same encryption
	let client_encryption = SimpleXorEncryption::new(encryption_key.clone());

	let client =
		WsClientBuilder::default().set_message_encryption(client_encryption).build(&format!("ws://{}", addr)).await?;

	println!("Client connected with encryption enabled");

	// Test encrypted communication
	let response: String = client.request("echo", vec!["Hello Encrypted World!"]).await?;
	println!("Encrypted echo response: {}", response);

	// Test with JSON object
	#[derive(serde::Serialize)]
	struct Transfer {
		amount: u64,
		to: String,
	}

	let transfer = Transfer { amount: 1000, to: "alice".to_string() };

	let response: String = client.request("echo", [serde_json::to_string(&transfer)?]).await?;
	println!("Encrypted transfer echo: {}", response);

	server_handle.stop()?;

	Ok(())
}
