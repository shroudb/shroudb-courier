/// Courier protocol commands.
#[derive(Debug, Clone)]
pub enum Command {
    /// Reload templates from disk.
    TemplateReload,

    /// List all loaded templates.
    TemplateList,

    /// Get information about a specific template.
    TemplateInfo { name: String },

    /// Deliver a notification (JSON payload).
    Deliver { json: String },

    /// Health check.
    Health,

    /// Get a config value by key.
    ConfigGet { key: String },

    /// Set a config value.
    ConfigSet { key: String, value: String },

    /// List all config entries.
    ConfigList,

    /// Authenticate the connection.
    Auth { token: String },

    /// Execute a batch of commands.
    Pipeline(Vec<Command>),
}

/// Get the verb string for a command (for metrics and audit logging).
pub fn command_verb(cmd: &Command) -> &'static str {
    match cmd {
        Command::TemplateReload => "TEMPLATE_RELOAD",
        Command::TemplateList => "TEMPLATE_LIST",
        Command::TemplateInfo { .. } => "TEMPLATE_INFO",
        Command::Deliver { .. } => "DELIVER",
        Command::Health => "HEALTH",
        Command::ConfigGet { .. } => "CONFIG",
        Command::ConfigSet { .. } => "CONFIG",
        Command::ConfigList => "CONFIG",
        Command::Auth { .. } => "AUTH",
        Command::Pipeline(_) => "PIPELINE",
    }
}

impl Command {
    /// Whether this is a read-only command.
    pub fn is_read(&self) -> bool {
        matches!(
            self,
            Command::TemplateList
                | Command::TemplateInfo { .. }
                | Command::Health
                | Command::ConfigGet { .. }
                | Command::ConfigList
        )
    }
}
