//! Parse a list of string tokens into a Courier `Command`.

use crate::command::Command;
use crate::error::CommandError;

pub fn parse_command(strings: Vec<String>) -> Result<Command, CommandError> {
    if strings.is_empty() {
        return Err(CommandError::BadArg {
            message: "empty command".into(),
        });
    }

    let verb = strings[0].to_ascii_uppercase();
    let args = &strings[1..];

    match verb.as_str() {
        "TEMPLATE_RELOAD" => Ok(Command::TemplateReload),
        "TEMPLATE_LIST" => Ok(Command::TemplateList),
        "TEMPLATE_INFO" => parse_template_info(args),
        "DELIVER" => parse_deliver(args),
        "HEALTH" => Ok(Command::Health),
        "AUTH" => parse_auth(args),
        "PIPELINE" => parse_pipeline(&strings),
        _ => Err(CommandError::BadArg {
            message: format!("unknown command: {verb}"),
        }),
    }
}

fn parse_template_info(args: &[String]) -> Result<Command, CommandError> {
    require_arg(args, "TEMPLATE_INFO", 1)?;
    Ok(Command::TemplateInfo {
        name: args[0].clone(),
    })
}

fn parse_deliver(args: &[String]) -> Result<Command, CommandError> {
    require_arg(args, "DELIVER", 1)?;
    Ok(Command::Deliver {
        json: args[0].clone(),
    })
}

fn parse_auth(args: &[String]) -> Result<Command, CommandError> {
    require_arg(args, "AUTH", 1)?;
    Ok(Command::Auth {
        token: args[0].clone(),
    })
}

fn parse_pipeline(strings: &[String]) -> Result<Command, CommandError> {
    let tokens = &strings[1..];
    let mut commands = Vec::new();
    let mut current = Vec::new();

    for token in tokens {
        if token.eq_ignore_ascii_case("END") {
            if current.is_empty() {
                continue;
            }
            commands.push(parse_command(std::mem::take(&mut current))?);
        } else {
            current.push(token.clone());
        }
    }

    if !current.is_empty() {
        commands.push(parse_command(current)?);
    }

    if commands.is_empty() {
        return Err(CommandError::BadArg {
            message: "PIPELINE contains no commands".into(),
        });
    }

    Ok(Command::Pipeline(commands))
}

fn require_arg(args: &[String], cmd: &str, min: usize) -> Result<(), CommandError> {
    if args.len() < min {
        return Err(CommandError::BadArg {
            message: format!("{cmd} requires at least {min} argument(s)"),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(strings: &[&str]) -> Vec<String> {
        strings.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parse_template_list() {
        let cmd = parse_command(s(&["TEMPLATE_LIST"])).unwrap();
        assert!(matches!(cmd, Command::TemplateList));
    }

    #[test]
    fn parse_template_info_basic() {
        let cmd = parse_command(s(&["TEMPLATE_INFO", "welcome"])).unwrap();
        match cmd {
            Command::TemplateInfo { name } => assert_eq!(name, "welcome"),
            _ => panic!("expected TemplateInfo"),
        }
    }

    #[test]
    fn parse_template_reload() {
        let cmd = parse_command(s(&["TEMPLATE_RELOAD"])).unwrap();
        assert!(matches!(cmd, Command::TemplateReload));
    }

    #[test]
    fn parse_deliver_json() {
        let json = r#"{"channel":"email","recipient":"enc...","body":"hello"}"#;
        let cmd = parse_command(s(&["DELIVER", json])).unwrap();
        match cmd {
            Command::Deliver { json: j } => assert_eq!(j, json),
            _ => panic!("expected Deliver"),
        }
    }

    #[test]
    fn parse_health() {
        let cmd = parse_command(s(&["HEALTH"])).unwrap();
        assert!(matches!(cmd, Command::Health));
    }

    #[test]
    fn parse_auth() {
        let cmd = parse_command(s(&["AUTH", "my-token"])).unwrap();
        match cmd {
            Command::Auth { token } => assert_eq!(token, "my-token"),
            _ => panic!("expected Auth"),
        }
    }

    #[test]
    fn parse_unknown_command() {
        let result = parse_command(s(&["BOGUS"]));
        assert!(result.is_err());
    }

    #[test]
    fn parse_empty_command() {
        let result = parse_command(vec![]);
        assert!(result.is_err());
    }

    #[test]
    fn parse_pipeline_commands() {
        let cmd = parse_command(s(&["PIPELINE", "TEMPLATE_LIST", "END", "HEALTH", "END"])).unwrap();
        match cmd {
            Command::Pipeline(cmds) => {
                assert_eq!(cmds.len(), 2);
                assert!(matches!(cmds[0], Command::TemplateList));
                assert!(matches!(cmds[1], Command::Health));
            }
            _ => panic!("expected Pipeline"),
        }
    }
}
