use shroudb_courier_core::template::TemplateEngine;

use crate::error::CommandError;
use crate::response::{ResponseMap, ResponseValue};

pub fn handle_template_list(engine: &TemplateEngine) -> Result<ResponseMap, CommandError> {
    let templates = engine.list();
    let names: Vec<ResponseValue> = templates
        .iter()
        .map(|t| ResponseValue::String(t.name.clone()))
        .collect();
    Ok(ResponseMap::ok()
        .with("count", ResponseValue::Integer(names.len() as i64))
        .with("templates", ResponseValue::Array(names)))
}
