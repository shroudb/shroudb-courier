use anyhow::Result;
use clap::Parser;
use shroudb_courier_client::CourierClient;
use tokio::io::AsyncBufReadExt;

#[derive(Parser)]
#[command(name = "shroudb-courier-cli", about = "CLI for ShrouDB Courier")]
struct Cli {
    #[arg(long, default_value = "127.0.0.1:6999", env = "COURIER_ADDR")]
    addr: String,

    #[arg(trailing_var_arg = true)]
    command: Vec<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    if cli.command.is_empty() {
        interactive(&cli.addr).await
    } else {
        let mut client = CourierClient::connect(&cli.addr).await?;
        execute(&mut client, &cli.command).await
    }
}

async fn interactive(addr: &str) -> Result<()> {
    let mut client = CourierClient::connect(addr).await?;
    let stdin = tokio::io::stdin();
    let reader = tokio::io::BufReader::new(stdin);
    let mut lines = reader.lines();

    eprint!("courier> ");
    while let Some(line) = lines.next_line().await? {
        let line: String = line.trim().to_string();
        if line.is_empty() {
            eprint!("courier> ");
            continue;
        }
        if line == "quit" || line == "exit" {
            break;
        }

        let parts = shell_split(&line);
        match execute(&mut client, &parts).await {
            Ok(()) => {}
            Err(e) => eprintln!("error: {e}"),
        }
        eprint!("courier> ");
    }

    Ok(())
}

async fn execute(client: &mut CourierClient, args: &[String]) -> Result<()> {
    if args.is_empty() {
        return Ok(());
    }

    let verb = args[0].to_uppercase();
    let result: Result<serde_json::Value> = match verb.as_str() {
        "HEALTH" => client.health().await.map_err(Into::into),
        "PING" => {
            let _resp = client.health().await?;
            println!("PONG");
            return Ok(());
        }
        "AUTH" => {
            if args.len() < 2 {
                anyhow::bail!("AUTH requires a token");
            }
            client.auth(&args[1]).await?;
            println!("OK");
            return Ok(());
        }

        "CHANNEL" => {
            if args.len() < 2 {
                anyhow::bail!("CHANNEL requires a subcommand");
            }
            let sub = args[1].to_uppercase();
            match sub.as_str() {
                "CREATE" => {
                    if args.len() < 5 {
                        anyhow::bail!("CHANNEL CREATE <name> <type> <config_json>");
                    }
                    client
                        .channel_create(&args[2], &args[3], &args[4])
                        .await
                        .map_err(Into::into)
                }
                "GET" => {
                    if args.len() < 3 {
                        anyhow::bail!("CHANNEL GET <name>");
                    }
                    client.channel_get(&args[2]).await.map_err(Into::into)
                }
                "LIST" => client.channel_list().await.map_err(Into::into),
                "DELETE" => {
                    if args.len() < 3 {
                        anyhow::bail!("CHANNEL DELETE <name>");
                    }
                    client.channel_delete(&args[2]).await.map_err(Into::into)
                }
                other => anyhow::bail!("unknown CHANNEL subcommand: {other}"),
            }
        }

        "DELIVER" => {
            if args.len() < 2 {
                anyhow::bail!("DELIVER requires a JSON request");
            }
            client.deliver(&args[1]).await.map_err(Into::into)
        }

        "COMMAND" => {
            if args.len() >= 2 && args[1].to_uppercase() == "LIST" {
                println!("Available commands:");
                println!("  AUTH <token>");
                println!("  CHANNEL CREATE|GET|LIST|DELETE");
                println!("  DELIVER <json>");
                println!("  HEALTH");
                println!("  PING");
                println!("  COMMAND LIST");
                return Ok(());
            }
            anyhow::bail!("unknown COMMAND subcommand");
        }

        other => anyhow::bail!("unknown command: {other}"),
    };

    match result {
        Ok(value) => {
            println!("{}", serde_json::to_string_pretty(&value)?);
        }
        Err(e) => {
            eprintln!("error: {e}");
        }
    }

    Ok(())
}

fn shell_split(s: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut quote_char = ' ';
    let mut brace_depth: i32 = 0;
    let mut bracket_depth: i32 = 0;

    for ch in s.chars() {
        if in_quotes {
            if ch == quote_char {
                in_quotes = false;
            } else {
                current.push(ch);
            }
            continue;
        }

        match ch {
            '"' | '\'' => {
                in_quotes = true;
                quote_char = ch;
            }
            '{' => {
                brace_depth += 1;
                current.push(ch);
            }
            '}' => {
                brace_depth -= 1;
                current.push(ch);
            }
            '[' => {
                bracket_depth += 1;
                current.push(ch);
            }
            ']' => {
                bracket_depth -= 1;
                current.push(ch);
            }
            ' ' | '\t' if brace_depth == 0 && bracket_depth == 0 => {
                if !current.is_empty() {
                    result.push(current.clone());
                    current.clear();
                }
            }
            _ => {
                current.push(ch);
            }
        }
    }

    if !current.is_empty() {
        result.push(current);
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shell_split_simple() {
        assert_eq!(
            shell_split("CHANNEL CREATE test email"),
            vec!["CHANNEL", "CREATE", "test", "email"]
        );
    }

    #[test]
    fn test_shell_split_json() {
        let input = r#"DELIVER {"channel":"email","recipient":"enc:x","body":"hello"}"#;
        let parts = shell_split(input);
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0], "DELIVER");
        assert!(parts[1].starts_with('{'));
    }

    #[test]
    fn test_shell_split_quoted() {
        assert_eq!(shell_split(r#"AUTH "my token""#), vec!["AUTH", "my token"]);
    }

    #[test]
    fn test_shell_split_empty() {
        assert!(shell_split("").is_empty());
        assert!(shell_split("   ").is_empty());
    }
}
