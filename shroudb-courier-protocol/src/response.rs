use serde_json::Value;

#[derive(Debug, Clone)]
pub enum CourierResponse {
    Ok(Value),
    Error(String),
}

impl CourierResponse {
    pub fn ok(value: Value) -> Self {
        CourierResponse::Ok(value)
    }

    pub fn ok_simple() -> Self {
        CourierResponse::Ok(serde_json::json!({"status": "ok"}))
    }

    pub fn error(msg: impl Into<String>) -> Self {
        CourierResponse::Error(msg.into())
    }

    pub fn is_ok(&self) -> bool {
        matches!(self, CourierResponse::Ok(_))
    }
}
