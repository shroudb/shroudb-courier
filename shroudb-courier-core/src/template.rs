//! Template engine — loads Tera templates from a directory and renders messages.

use std::collections::HashMap;
use std::path::Path;

use crate::delivery::{ContentType, RenderedMessage};
use crate::error::CourierError;

/// Information about a loaded template.
#[derive(Debug, Clone)]
pub struct TemplateInfo {
    pub name: String,
    pub has_subject: bool,
    pub has_html_body: bool,
    pub has_text_body: bool,
}

/// Template engine wrapping Tera.
///
/// Templates are loaded from a directory following the naming convention:
///   - `{name}.subject.txt`  — subject line template
///   - `{name}.body.html`    — HTML body template
///   - `{name}.body.txt`     — plain text body template
pub struct TemplateEngine {
    tera: tera::Tera,
    templates: HashMap<String, TemplateInfo>,
}

impl TemplateEngine {
    /// Load templates from a directory. Scans for `{name}.subject.txt`,
    /// `{name}.body.html`, and `{name}.body.txt` patterns.
    pub fn load_dir(path: &Path) -> Result<Self, CourierError> {
        let mut tera = tera::Tera::default();
        let mut template_names: HashMap<String, TemplateInfo> = HashMap::new();

        if !path.exists() {
            tracing::warn!(dir = %path.display(), "templates directory does not exist");
            return Ok(Self {
                tera,
                templates: template_names,
            });
        }

        let entries = std::fs::read_dir(path)
            .map_err(|e| CourierError::TemplateRenderFailed(format!("cannot read dir: {e}")))?;

        for entry in entries {
            let entry = entry
                .map_err(|e| CourierError::TemplateRenderFailed(format!("dir entry error: {e}")))?;
            let file_name = entry.file_name().to_string_lossy().to_string();
            let file_path = entry.path();

            if !file_path.is_file() {
                continue;
            }

            // Parse template file name: {name}.{kind}
            let (base_name, kind) = if let Some(name) = file_name.strip_suffix(".subject.txt") {
                (name.to_string(), "subject")
            } else if let Some(name) = file_name.strip_suffix(".body.html") {
                (name.to_string(), "html_body")
            } else if let Some(name) = file_name.strip_suffix(".body.txt") {
                (name.to_string(), "text_body")
            } else {
                continue;
            };

            let content = std::fs::read_to_string(&file_path).map_err(|e| {
                CourierError::TemplateRenderFailed(format!(
                    "failed to read {}: {e}",
                    file_path.display()
                ))
            })?;

            // Register in Tera with a namespaced key: "{name}/{kind}"
            let tera_name = format!("{base_name}/{kind}");
            tera.add_raw_template(&tera_name, &content).map_err(|e| {
                CourierError::TemplateRenderFailed(format!("tera parse error: {e}"))
            })?;

            let info = template_names
                .entry(base_name.clone())
                .or_insert_with(|| TemplateInfo {
                    name: base_name.clone(),
                    has_subject: false,
                    has_html_body: false,
                    has_text_body: false,
                });

            match kind {
                "subject" => info.has_subject = true,
                "html_body" => info.has_html_body = true,
                "text_body" => info.has_text_body = true,
                _ => {}
            }
        }

        tracing::info!(
            count = template_names.len(),
            dir = %path.display(),
            "templates loaded"
        );

        Ok(Self {
            tera,
            templates: template_names,
        })
    }

    /// Render a template with the given variables.
    pub fn render(
        &self,
        name: &str,
        vars: &HashMap<String, serde_json::Value>,
    ) -> Result<RenderedMessage, CourierError> {
        let info = self
            .templates
            .get(name)
            .ok_or_else(|| CourierError::TemplateNotFound(name.to_string()))?;

        let mut context = tera::Context::new();
        for (k, v) in vars {
            context.insert(k, v);
        }

        // Render subject if available.
        let subject = if info.has_subject {
            let key = format!("{name}/subject");
            Some(
                self.tera
                    .render(&key, &context)
                    .map_err(|e| CourierError::TemplateRenderFailed(e.to_string()))?,
            )
        } else {
            None
        };

        // Prefer HTML body, fall back to text body.
        let (body, content_type) = if info.has_html_body {
            let key = format!("{name}/html_body");
            let rendered = self
                .tera
                .render(&key, &context)
                .map_err(|e| CourierError::TemplateRenderFailed(e.to_string()))?;
            (rendered, ContentType::Html)
        } else if info.has_text_body {
            let key = format!("{name}/text_body");
            let rendered = self
                .tera
                .render(&key, &context)
                .map_err(|e| CourierError::TemplateRenderFailed(e.to_string()))?;
            (rendered, ContentType::Plain)
        } else {
            return Err(CourierError::TemplateRenderFailed(format!(
                "template '{name}' has no body file"
            )));
        };

        Ok(RenderedMessage {
            subject,
            body,
            content_type,
        })
    }

    /// Hot-reload templates from directory. Returns the count of loaded templates.
    pub fn reload(&mut self, path: &Path) -> Result<usize, CourierError> {
        let reloaded = Self::load_dir(path)?;
        self.tera = reloaded.tera;
        self.templates = reloaded.templates;
        Ok(self.templates.len())
    }

    /// List all loaded templates.
    pub fn list(&self) -> Vec<&TemplateInfo> {
        self.templates.values().collect()
    }

    /// Get a specific template info by name.
    pub fn get(&self, name: &str) -> Option<&TemplateInfo> {
        self.templates.get(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn setup_templates(dir: &Path) {
        fs::create_dir_all(dir).unwrap();
        fs::write(dir.join("welcome.subject.txt"), "Welcome, {{ user_name }}!").unwrap();
        fs::write(
            dir.join("welcome.body.html"),
            "<h1>Hello {{ user_name }}</h1><p>Welcome to {{ service }}.</p>",
        )
        .unwrap();
        fs::write(dir.join("alert.body.txt"), "Alert: {{ message }}").unwrap();
    }

    #[test]
    fn load_and_render_template() {
        let dir = std::env::temp_dir().join("courier_test_templates_load");
        let _ = fs::remove_dir_all(&dir);
        setup_templates(&dir);

        let engine = TemplateEngine::load_dir(&dir).unwrap();

        // Check template list.
        assert_eq!(engine.list().len(), 2);

        // Check welcome template.
        let info = engine.get("welcome").unwrap();
        assert!(info.has_subject);
        assert!(info.has_html_body);
        assert!(!info.has_text_body);

        // Render.
        let mut vars = HashMap::new();
        vars.insert("user_name".into(), serde_json::json!("Alice"));
        vars.insert("service".into(), serde_json::json!("Courier"));
        let msg = engine.render("welcome", &vars).unwrap();
        assert_eq!(msg.subject.as_deref(), Some("Welcome, Alice!"));
        assert!(msg.body.contains("Hello Alice"));
        assert_eq!(msg.content_type, ContentType::Html);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn render_text_only_template() {
        let dir = std::env::temp_dir().join("courier_test_templates_text");
        let _ = fs::remove_dir_all(&dir);
        setup_templates(&dir);

        let engine = TemplateEngine::load_dir(&dir).unwrap();
        let mut vars = HashMap::new();
        vars.insert("message".into(), serde_json::json!("CPU at 99%"));
        let msg = engine.render("alert", &vars).unwrap();
        assert!(msg.subject.is_none());
        assert_eq!(msg.body, "Alert: CPU at 99%");
        assert_eq!(msg.content_type, ContentType::Plain);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_template_returns_error() {
        let dir = std::env::temp_dir().join("courier_test_templates_missing");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let engine = TemplateEngine::load_dir(&dir).unwrap();
        let result = engine.render("nonexistent", &HashMap::new());
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            CourierError::TemplateNotFound(_)
        ));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_variable_returns_error() {
        let dir = std::env::temp_dir().join("courier_test_templates_missingvar");
        let _ = fs::remove_dir_all(&dir);
        setup_templates(&dir);

        let engine = TemplateEngine::load_dir(&dir).unwrap();
        // Render welcome without providing user_name — tera strict mode should error.
        let result = engine.render("welcome", &HashMap::new());
        assert!(result.is_err());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn reload_templates() {
        let dir = std::env::temp_dir().join("courier_test_templates_reload");
        let _ = fs::remove_dir_all(&dir);
        setup_templates(&dir);

        let mut engine = TemplateEngine::load_dir(&dir).unwrap();
        assert_eq!(engine.list().len(), 2);

        // Add a new template.
        fs::write(dir.join("new.body.txt"), "New template: {{ x }}").unwrap();
        let count = engine.reload(&dir).unwrap();
        assert_eq!(count, 3);

        let _ = fs::remove_dir_all(&dir);
    }
}
