use shroudb_courier_core::template::TemplateEngine;

use crate::error::CommandError;
use crate::response::{ResponseMap, ResponseValue};

pub fn handle_template_info(
    engine: &TemplateEngine,
    name: &str,
) -> Result<ResponseMap, CommandError> {
    let info = engine
        .get(name)
        .ok_or_else(|| CommandError::TemplateNotFound(name.to_string()))?;

    Ok(ResponseMap::ok()
        .with("name", ResponseValue::String(info.name.clone()))
        .with("has_subject", ResponseValue::Boolean(info.has_subject))
        .with("has_html_body", ResponseValue::Boolean(info.has_html_body))
        .with("has_text_body", ResponseValue::Boolean(info.has_text_body)))
}
