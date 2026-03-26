use shroudb_courier_core::adapter::AdapterRegistry;
use shroudb_courier_core::delivery::{ContentType, DeliveryRequest, RenderedMessage};
use shroudb_courier_core::template::TemplateEngine;
use shroudb_courier_core::transit::TransitDecryptor;
use zeroize::Zeroize;

use crate::error::CommandError;
use crate::response::{ResponseMap, ResponseValue};

/// Execute the delivery pipeline:
/// 1. Parse the JSON delivery request
/// 2. Decrypt recipient via Transit
/// 3. Render message (template or pre-rendered)
/// 4. Find adapter for channel
/// 5. Deliver
/// 6. Zeroize plaintext
/// 7. Return receipt
pub async fn handle_deliver(
    json: &str,
    template_engine: &TemplateEngine,
    adapters: &AdapterRegistry,
    transit: &TransitDecryptor,
) -> Result<ResponseMap, CommandError> {
    // 1. Parse request.
    let request: DeliveryRequest = serde_json::from_str(json)
        .map_err(|e| CommandError::InvalidRequest(format!("invalid JSON: {e}")))?;

    // 2. Decrypt recipient.
    let plaintext_secret = transit.decrypt(&request.recipient).await?;
    let mut plaintext_recipient =
        String::from_utf8(plaintext_secret.as_bytes().to_vec()).map_err(|_| {
            CommandError::InvalidRequest("decrypted recipient is not valid UTF-8".into())
        })?;

    // 3. Render message.
    let message = if let Some(ref template_name) = request.template {
        let vars = request.vars.as_ref().cloned().unwrap_or_default();
        template_engine.render(template_name, &vars)?
    } else if let Some(ref body) = request.body {
        RenderedMessage {
            subject: request.subject.clone(),
            body: body.clone(),
            content_type: ContentType::Plain,
        }
    } else {
        return Err(CommandError::InvalidRequest(
            "either 'template' or 'body' must be provided".into(),
        ));
    };

    // 4. Find adapter.
    let adapter = adapters.get(request.channel).ok_or_else(|| {
        CommandError::AdapterNotFound(format!("no adapter for channel: {}", request.channel))
    })?;

    // 5. Deliver.
    let receipt = adapter.deliver(&plaintext_recipient, &message).await?;

    // 6. Zeroize plaintext.
    plaintext_recipient.zeroize();
    // SecretBytes (from transit.decrypt) zeroizes on drop automatically.

    // 7. Return receipt.
    Ok(ResponseMap::ok()
        .with("delivery_id", ResponseValue::String(receipt.delivery_id))
        .with(
            "channel",
            ResponseValue::String(receipt.channel.to_string()),
        )
        .with("adapter", ResponseValue::String(receipt.adapter))
        .with("status", ResponseValue::String(receipt.status.to_string()))
        .with(
            "delivered_at",
            ResponseValue::Integer(receipt.delivered_at as i64),
        )
        .with(
            "error",
            match receipt.error {
                Some(e) => ResponseValue::String(e),
                None => ResponseValue::Null,
            },
        )
        .with(
            "recipients",
            match receipt.recipients {
                Some(n) => ResponseValue::Integer(n as i64),
                None => ResponseValue::Null,
            },
        ))
}
