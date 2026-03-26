//! shroudb-courier-cli — interactive command-line client for ShrouDB Courier.

use clap::Parser;
use rustyline::error::ReadlineError;
use rustyline::hint::HistoryHinter;
use shroudb_courier_client::Response;
use shroudb_courier_client::connection::Connection;

/// Known command names for tab completion.
const COMMANDS: &[&str] = &[
    "TEMPLATE_RELOAD",
    "TEMPLATE_LIST",
    "TEMPLATE_INFO",
    "DELIVER",
    "HEALTH",
    "AUTH",
    "help",
    "quit",
    "exit",
];

// ---------------------------------------------------------------------------
// Tab-completion helper
// ---------------------------------------------------------------------------

struct CourierHelper {
    hinter: HistoryHinter,
}

impl rustyline::completion::Completer for CourierHelper {
    type Candidate = String;

    fn complete(
        &self,
        line: &str,
        pos: usize,
        _ctx: &rustyline::Context<'_>,
    ) -> rustyline::Result<(usize, Vec<String>)> {
        let word_start = line[..pos].rfind(' ').map(|i| i + 1).unwrap_or(0);
        let prefix = &line[word_start..pos];
        let matches: Vec<String> = COMMANDS
            .iter()
            .filter(|c| c.to_uppercase().starts_with(&prefix.to_uppercase()))
            .map(|c| c.to_string())
            .collect();
        Ok((word_start, matches))
    }
}

impl rustyline::hint::Hinter for CourierHelper {
    type Hint = String;

    fn hint(&self, line: &str, pos: usize, ctx: &rustyline::Context<'_>) -> Option<String> {
        self.hinter.hint(line, pos, ctx)
    }
}

impl rustyline::highlight::Highlighter for CourierHelper {}
impl rustyline::validate::Validator for CourierHelper {}
impl rustyline::Helper for CourierHelper {}

// ---------------------------------------------------------------------------
// CLI args
// ---------------------------------------------------------------------------

#[derive(Parser)]
#[command(
    name = "shroudb-courier-cli",
    about = "Interactive client for ShrouDB Courier",
    version
)]
struct Cli {
    /// Connection URI (e.g., shroudb-courier://localhost:6999, shroudb-courier+tls://token@host:6999).
    #[arg(long)]
    uri: Option<String>,

    /// Server host.
    #[arg(long, default_value = "127.0.0.1")]
    host: String,

    /// Server port.
    #[arg(short, long, default_value_t = 6999)]
    port: u16,

    /// Output responses as JSON.
    #[arg(long)]
    json: bool,

    /// Output raw wire format instead of parsed responses.
    #[arg(long)]
    raw: bool,

    /// Connect with TLS.
    #[arg(long)]
    tls: bool,

    /// Execute a single command and exit (non-interactive).
    #[arg(trailing_var_arg = true)]
    command: Vec<String>,
}

/// Output mode derived from CLI flags.
#[derive(Clone, Copy)]
enum OutputMode {
    Human,
    Json,
    Raw,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let output_mode = if cli.raw {
        OutputMode::Raw
    } else if cli.json {
        OutputMode::Json
    } else {
        OutputMode::Human
    };

    let (addr, mut conn) = if let Some(ref uri) = cli.uri {
        let config = shroudb_courier_client::parse_uri(uri)?;
        let addr = format!("{}:{}", config.host, config.port);
        let mut conn = if config.tls {
            Connection::connect_tls(&addr).await?
        } else {
            Connection::connect(&addr).await?
        };
        if let Some(token) = &config.auth_token {
            let auth_args = vec!["AUTH".to_string(), token.clone()];
            conn.send_command(&auth_args).await?;
        }
        (addr, conn)
    } else {
        let addr = format!("{}:{}", cli.host, cli.port);
        let conn = if cli.tls {
            Connection::connect_tls(&addr).await?
        } else {
            Connection::connect(&addr).await?
        };
        (addr, conn)
    };

    // Non-interactive: execute single command and exit.
    if !cli.command.is_empty() {
        let response = conn.send_command(&cli.command).await?;
        print_output(&response, output_mode);
        return Ok(());
    }

    // Interactive REPL.
    println!("Connected to shroudb-courier at {addr}");
    println!("Type 'help' for command list, 'help <command>' for details, Ctrl-C to exit.\n");

    let config = rustyline::Config::builder().auto_add_history(true).build();
    let helper = CourierHelper {
        hinter: HistoryHinter::new(),
    };
    let mut rl = rustyline::Editor::with_config(config)?;
    rl.set_helper(Some(helper));

    let history_path = dirs_home().join(".courier_history");
    let _ = rl.load_history(&history_path);

    loop {
        match rl.readline("courier> ") {
            Ok(line) => {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }

                if let Some(cmd) = line
                    .strip_prefix("help ")
                    .or_else(|| line.strip_prefix("HELP "))
                {
                    print_command_help(cmd.trim());
                    continue;
                }

                if line.eq_ignore_ascii_case("help") {
                    print_help();
                    continue;
                }
                if line.eq_ignore_ascii_case("quit") || line.eq_ignore_ascii_case("exit") {
                    break;
                }

                let args = shell_words(line);
                match conn.send_command(&args).await {
                    Ok(response) => print_output(&response, output_mode),
                    Err(e) => eprintln!("error: {e}"),
                }
            }
            Err(ReadlineError::Interrupted) => break,
            Err(ReadlineError::Eof) => break,
            Err(e) => {
                eprintln!("readline error: {e}");
                break;
            }
        }
    }

    let _ = rl.save_history(&history_path);
    Ok(())
}

/// Print a response in the requested output mode.
fn print_output(resp: &Response, mode: OutputMode) {
    match mode {
        OutputMode::Human => resp.print(0),
        OutputMode::Json => {
            let json_val = resp.to_json();
            println!("{}", serde_json::to_string_pretty(&json_val).unwrap());
        }
        OutputMode::Raw => {
            let raw = resp.to_raw();
            print!("{raw}");
        }
    }
}

fn dirs_home() -> std::path::PathBuf {
    std::env::var("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("."))
}

/// Split a line into words, respecting double-quoted strings.
fn shell_words(input: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;

    for ch in input.chars() {
        match ch {
            '"' => in_quotes = !in_quotes,
            ' ' | '\t' if !in_quotes => {
                if !current.is_empty() {
                    words.push(std::mem::take(&mut current));
                }
            }
            _ => current.push(ch),
        }
    }
    if !current.is_empty() {
        words.push(current);
    }
    words
}

fn print_help() {
    println!(
        r#"
Commands:

  Template Management
    TEMPLATE_RELOAD                     Reload all templates from disk
    TEMPLATE_LIST                       List all loaded templates
    TEMPLATE_INFO <name>                Get details of a specific template

  Delivery
    DELIVER <json>                      Deliver a notification

  Operational
    HEALTH                              Check server health
    AUTH <token>                        Authenticate the connection

  Other
    help [<command>]   Show help (optionally for a specific command)
    quit/exit          Disconnect
"#
    );
}

fn print_command_help(cmd: &str) {
    match cmd.to_uppercase().as_str() {
        "TEMPLATE_RELOAD" => println!(
            r#"TEMPLATE_RELOAD

  Reload all templates from disk.
  This re-reads all template files from the templates directory.

  Example:
    TEMPLATE_RELOAD
"#
        ),
        "TEMPLATE_LIST" => println!(
            r#"TEMPLATE_LIST

  List all loaded templates by name.

  Example:
    TEMPLATE_LIST
"#
        ),
        "TEMPLATE_INFO" => println!(
            r#"TEMPLATE_INFO <name>

  Show detailed information about a specific template.

  Example:
    TEMPLATE_INFO welcome
"#
        ),
        "DELIVER" => println!(
            r#"DELIVER <json>

  Deliver a notification. The JSON payload must include at minimum:
    - channel: "email", "webhook", "sms", or "push"
    - recipient: Transit-encrypted recipient ciphertext
    - template or body: template name (with optional vars) or pre-rendered body

  Example:
    DELIVER {{"channel":"email","recipient":"enc:...","template":"welcome","vars":{{"user_name":"Alice"}}}}
    DELIVER {{"channel":"webhook","recipient":"enc:...","body":"Alert fired"}}
"#
        ),
        "HEALTH" => println!(
            r#"HEALTH

  Check server health.

  Example:
    HEALTH
"#
        ),
        "AUTH" => println!(
            r#"AUTH <token>

  Authenticate the current connection with a bearer token.

  Example:
    AUTH my-secret-token
"#
        ),
        _ => println!("Unknown command: {cmd}. Type 'help' for all commands."),
    }
}
