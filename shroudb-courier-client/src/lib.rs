mod connection;
mod error;

pub use error::ClientError;

use connection::Connection;
use serde_json::Value;

pub struct CourierClient {
    conn: Connection,
}

impl CourierClient {
    /// Connect directly to a standalone Courier server.
    pub async fn connect(addr: &str) -> Result<Self, ClientError> {
        let conn = Connection::connect(addr).await?;
        Ok(Self { conn })
    }

    /// Connect to a Courier engine through a Moat gateway.
    ///
    /// Commands are automatically prefixed with `COURIER` for Moat routing.
    /// Meta-commands (AUTH, HEALTH, PING) are sent without prefix.
    pub async fn connect_moat(addr: &str) -> Result<Self, ClientError> {
        let conn = Connection::connect_moat(addr).await?;
        Ok(Self { conn })
    }

    pub async fn auth(&mut self, token: &str) -> Result<(), ClientError> {
        let resp = self.conn.send_meta_command(&["AUTH", token]).await?;
        check_status(&resp)?;
        Ok(())
    }

    pub async fn health(&mut self) -> Result<Value, ClientError> {
        self.conn.send_meta_command(&["HEALTH"]).await
    }

    // --- Channel operations ---

    pub async fn channel_create(
        &mut self,
        name: &str,
        channel_type: &str,
        config_json: &str,
    ) -> Result<Value, ClientError> {
        let resp = self
            .conn
            .send_command(&["CHANNEL", "CREATE", name, channel_type, config_json])
            .await?;
        check_status(&resp)?;
        Ok(resp)
    }

    pub async fn channel_get(&mut self, name: &str) -> Result<Value, ClientError> {
        self.conn.send_command(&["CHANNEL", "GET", name]).await
    }

    pub async fn channel_list(&mut self) -> Result<Value, ClientError> {
        self.conn.send_command(&["CHANNEL", "LIST"]).await
    }

    pub async fn channel_delete(&mut self, name: &str) -> Result<Value, ClientError> {
        let resp = self.conn.send_command(&["CHANNEL", "DELETE", name]).await?;
        check_status(&resp)?;
        Ok(resp)
    }

    // --- Delivery ---

    pub async fn deliver(&mut self, request_json: &str) -> Result<Value, ClientError> {
        self.conn.send_command(&["DELIVER", request_json]).await
    }
}

fn check_status(value: &Value) -> Result<(), ClientError> {
    if value
        .get("status")
        .and_then(|s| s.as_str())
        .is_some_and(|status| status == "ok" || status == "delivered")
    {
        return Ok(());
    }
    if value.is_object() || value.is_array() {
        return Ok(());
    }
    Ok(())
}
