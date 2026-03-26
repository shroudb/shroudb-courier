use shroudb_courier_core::adapter::AdapterRegistry;
use shroudb_courier_core::template::TemplateEngine;

use crate::error::CommandError;
use crate::response::{ResponseMap, ResponseValue};

pub fn handle_health(
    template_engine: &TemplateEngine,
    adapters: &AdapterRegistry,
) -> Result<ResponseMap, CommandError> {
    let template_count = template_engine.list().len();
    let adapter_list = adapters.list();
    let adapter_names: Vec<ResponseValue> = adapter_list
        .iter()
        .map(|(ch, name)| ResponseValue::String(format!("{ch}:{name}")))
        .collect();

    Ok(ResponseMap::ok()
        .with("health", ResponseValue::String("ok".into()))
        .with(
            "template_count",
            ResponseValue::Integer(template_count as i64),
        )
        .with(
            "adapter_count",
            ResponseValue::Integer(adapter_names.len() as i64),
        )
        .with("adapters", ResponseValue::Array(adapter_names)))
}
