use shroudb_acl::{AclRequirement, Scope};

#[derive(Debug, Clone)]
pub enum CourierCommand {
    Auth {
        token: String,
    },
    ChannelCreate {
        name: String,
        channel_type: String,
        config_json: String,
    },
    ChannelGet {
        name: String,
    },
    ChannelList,
    ChannelDelete {
        name: String,
    },
    Deliver {
        request_json: String,
    },
    NotifyEvent {
        channel: String,
        subject: String,
        body: String,
    },
    DeliveryGet {
        id: String,
    },
    DeliveryList {
        channel: Option<String>,
        limit: usize,
    },
    Metrics,
    Health,
    Ping,
    CommandList,
    /// Engine identity handshake. Pre-auth; returns engine name, version,
    /// wire protocol, supported commands, and capability tags so a client
    /// can detect SDK/engine version mismatches before issuing any real
    /// command.
    Hello,
}

impl CourierCommand {
    pub fn acl_requirement(&self) -> AclRequirement {
        match self {
            CourierCommand::Auth { .. }
            | CourierCommand::Health
            | CourierCommand::Ping
            | CourierCommand::CommandList
            | CourierCommand::Hello
            | CourierCommand::Metrics
            | CourierCommand::ChannelList => AclRequirement::None,

            CourierCommand::DeliveryGet { .. } | CourierCommand::DeliveryList { .. } => {
                AclRequirement::Admin
            }

            CourierCommand::ChannelCreate { .. } | CourierCommand::ChannelDelete { .. } => {
                AclRequirement::Admin
            }

            CourierCommand::ChannelGet { name } => AclRequirement::Namespace {
                ns: format!("courier.{name}.*"),
                scope: Scope::Read,
                tenant_override: None,
            },
            CourierCommand::NotifyEvent { channel, .. } => AclRequirement::Namespace {
                ns: format!("courier.{channel}.*"),
                scope: Scope::Write,
                tenant_override: None,
            },
            CourierCommand::Deliver { .. } => {
                let channel = match self {
                    CourierCommand::Deliver { request_json } => {
                        extract_channel(request_json).unwrap_or_default()
                    }
                    _ => unreachable!(),
                };
                AclRequirement::Namespace {
                    ns: format!("courier.{channel}.*"),
                    scope: Scope::Write,
                    tenant_override: None,
                }
            }
        }
    }
}

fn extract_channel(json: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(json)
        .ok()
        .and_then(|v| v.get("channel").and_then(|c| c.as_str()).map(String::from))
}

pub fn parse_command(args: &[&str]) -> Result<CourierCommand, String> {
    if args.is_empty() {
        return Err("empty command".into());
    }

    let verb = args[0].to_uppercase();
    match verb.as_str() {
        "AUTH" => {
            if args.len() < 2 {
                return Err("AUTH requires a token".into());
            }
            Ok(CourierCommand::Auth {
                token: args[1].to_string(),
            })
        }

        "CHANNEL" => {
            if args.len() < 2 {
                return Err("CHANNEL requires a subcommand (CREATE, GET, LIST, DELETE)".into());
            }
            let sub = args[1].to_uppercase();
            match sub.as_str() {
                "CREATE" => {
                    if args.len() < 5 {
                        return Err("CHANNEL CREATE requires <name> <type> <config_json>".into());
                    }
                    Ok(CourierCommand::ChannelCreate {
                        name: args[2].to_string(),
                        channel_type: args[3].to_string(),
                        config_json: args[4].to_string(),
                    })
                }
                "GET" => {
                    if args.len() < 3 {
                        return Err("CHANNEL GET requires <name>".into());
                    }
                    Ok(CourierCommand::ChannelGet {
                        name: args[2].to_string(),
                    })
                }
                "LIST" => Ok(CourierCommand::ChannelList),
                "DELETE" => {
                    if args.len() < 3 {
                        return Err("CHANNEL DELETE requires <name>".into());
                    }
                    Ok(CourierCommand::ChannelDelete {
                        name: args[2].to_string(),
                    })
                }
                other => Err(format!("unknown CHANNEL subcommand: {other}")),
            }
        }

        "NOTIFY_EVENT" => {
            if args.len() < 4 {
                return Err("NOTIFY_EVENT requires <channel> <subject> <body>".into());
            }
            Ok(CourierCommand::NotifyEvent {
                channel: args[1].to_string(),
                subject: args[2].to_string(),
                body: args[3].to_string(),
            })
        }

        "DELIVER" => {
            if args.len() < 2 {
                return Err("DELIVER requires a JSON request".into());
            }
            Ok(CourierCommand::Deliver {
                request_json: args[1].to_string(),
            })
        }

        "DELIVERY" => {
            if args.len() < 2 {
                return Err("DELIVERY requires a subcommand (GET, LIST)".into());
            }
            let sub = args[1].to_uppercase();
            match sub.as_str() {
                "GET" => {
                    if args.len() < 3 {
                        return Err("DELIVERY GET requires <id>".into());
                    }
                    Ok(CourierCommand::DeliveryGet {
                        id: args[2].to_string(),
                    })
                }
                "LIST" => {
                    let mut channel = None;
                    let mut limit = 100usize;
                    let mut i = 2;
                    while i < args.len() {
                        match args[i].to_uppercase().as_str() {
                            "CHANNEL" => {
                                if i + 1 < args.len() {
                                    channel = Some(args[i + 1].to_string());
                                    i += 2;
                                } else {
                                    return Err("CHANNEL requires a name".into());
                                }
                            }
                            "LIMIT" => {
                                if i + 1 < args.len() {
                                    limit = args[i + 1]
                                        .parse()
                                        .map_err(|_| "LIMIT must be a number".to_string())?;
                                    i += 2;
                                } else {
                                    return Err("LIMIT requires a number".into());
                                }
                            }
                            _ => {
                                return Err(format!("unknown DELIVERY LIST option: {}", args[i]));
                            }
                        }
                    }
                    Ok(CourierCommand::DeliveryList { channel, limit })
                }
                other => Err(format!("unknown DELIVERY subcommand: {other}")),
            }
        }

        "METRICS" => Ok(CourierCommand::Metrics),
        "HEALTH" => Ok(CourierCommand::Health),
        "PING" => Ok(CourierCommand::Ping),
        "HELLO" => Ok(CourierCommand::Hello),

        "COMMAND" => {
            if args.len() >= 2 && args[1].to_uppercase() == "LIST" {
                Ok(CourierCommand::CommandList)
            } else {
                Err("unknown COMMAND subcommand — did you mean COMMAND LIST?".into())
            }
        }

        other => Err(format!("unknown command: {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_auth() {
        let cmd = parse_command(&["AUTH", "my-token"]).unwrap();
        assert!(matches!(cmd, CourierCommand::Auth { token } if token == "my-token"));
    }

    #[test]
    fn test_parse_channel_create() {
        let cmd = parse_command(&["CHANNEL", "CREATE", "email-prod", "email", "{}"]).unwrap();
        assert!(
            matches!(cmd, CourierCommand::ChannelCreate { name, channel_type, .. }
            if name == "email-prod" && channel_type == "email")
        );
    }

    #[test]
    fn test_parse_channel_get() {
        let cmd = parse_command(&["CHANNEL", "GET", "email-prod"]).unwrap();
        assert!(matches!(cmd, CourierCommand::ChannelGet { name } if name == "email-prod"));
    }

    #[test]
    fn test_parse_channel_list() {
        let cmd = parse_command(&["CHANNEL", "LIST"]).unwrap();
        assert!(matches!(cmd, CourierCommand::ChannelList));
    }

    #[test]
    fn test_parse_channel_delete() {
        let cmd = parse_command(&["CHANNEL", "DELETE", "old"]).unwrap();
        assert!(matches!(cmd, CourierCommand::ChannelDelete { name } if name == "old"));
    }

    #[test]
    fn test_parse_notify_event() {
        let cmd = parse_command(&[
            "NOTIFY_EVENT",
            "rotation-alerts",
            "Key expiry",
            "Key X expires",
        ])
        .unwrap();
        match cmd {
            CourierCommand::NotifyEvent {
                channel,
                subject,
                body,
            } => {
                assert_eq!(channel, "rotation-alerts");
                assert_eq!(subject, "Key expiry");
                assert_eq!(body, "Key X expires");
            }
            _ => panic!("expected NotifyEvent"),
        }
    }

    #[test]
    fn test_parse_notify_event_missing_args() {
        assert!(parse_command(&["NOTIFY_EVENT"]).is_err());
        assert!(parse_command(&["NOTIFY_EVENT", "ch"]).is_err());
        assert!(parse_command(&["NOTIFY_EVENT", "ch", "subj"]).is_err());
    }

    #[test]
    fn test_acl_notify_event() {
        let cmd = CourierCommand::NotifyEvent {
            channel: "rotation-alerts".into(),
            subject: "test".into(),
            body: "test".into(),
        };
        match cmd.acl_requirement() {
            AclRequirement::Namespace { ns, scope, .. } => {
                assert_eq!(ns, "courier.rotation-alerts.*");
                assert_eq!(scope, Scope::Write);
            }
            _ => panic!("expected Namespace requirement"),
        }
    }

    #[test]
    fn test_parse_deliver() {
        let cmd = parse_command(&["DELIVER", "{\"channel\":\"email\"}"]).unwrap();
        assert!(matches!(cmd, CourierCommand::Deliver { .. }));
    }

    #[test]
    fn test_parse_health() {
        let cmd = parse_command(&["HEALTH"]).unwrap();
        assert!(matches!(cmd, CourierCommand::Health));
    }

    #[test]
    fn test_parse_ping() {
        let cmd = parse_command(&["PING"]).unwrap();
        assert!(matches!(cmd, CourierCommand::Ping));
    }

    #[test]
    fn test_parse_hello() {
        let cmd = parse_command(&["HELLO"]).unwrap();
        assert!(matches!(cmd, CourierCommand::Hello));
    }

    #[test]
    fn test_parse_command_list() {
        let cmd = parse_command(&["COMMAND", "LIST"]).unwrap();
        assert!(matches!(cmd, CourierCommand::CommandList));
    }

    #[test]
    fn test_parse_unknown() {
        assert!(parse_command(&["BOGUS"]).is_err());
    }

    #[test]
    fn test_parse_empty() {
        assert!(parse_command(&[]).is_err());
    }

    #[test]
    fn test_parse_delivery_get() {
        let cmd = parse_command(&["DELIVERY", "GET", "abc-123"]).unwrap();
        assert!(matches!(cmd, CourierCommand::DeliveryGet { id } if id == "abc-123"));
    }

    #[test]
    fn test_parse_delivery_list() {
        let cmd = parse_command(&["DELIVERY", "LIST"]).unwrap();
        assert!(matches!(
            cmd,
            CourierCommand::DeliveryList {
                channel: None,
                limit: 100
            }
        ));
    }

    #[test]
    fn test_parse_delivery_list_with_channel() {
        let cmd = parse_command(&["DELIVERY", "LIST", "CHANNEL", "email-prod"]).unwrap();
        assert!(
            matches!(cmd, CourierCommand::DeliveryList { channel: Some(c), limit: 100 } if c == "email-prod")
        );
    }

    #[test]
    fn test_parse_delivery_list_with_limit() {
        let cmd = parse_command(&["DELIVERY", "LIST", "LIMIT", "50"]).unwrap();
        assert!(matches!(
            cmd,
            CourierCommand::DeliveryList {
                channel: None,
                limit: 50
            }
        ));
    }

    #[test]
    fn test_parse_delivery_list_with_channel_and_limit() {
        let cmd =
            parse_command(&["DELIVERY", "LIST", "CHANNEL", "webhook", "LIMIT", "25"]).unwrap();
        assert!(
            matches!(cmd, CourierCommand::DeliveryList { channel: Some(c), limit: 25 } if c == "webhook")
        );
    }

    #[test]
    fn test_parse_delivery_missing_subcommand() {
        assert!(parse_command(&["DELIVERY"]).is_err());
    }

    #[test]
    fn test_parse_delivery_get_missing_id() {
        assert!(parse_command(&["DELIVERY", "GET"]).is_err());
    }

    #[test]
    fn test_parse_metrics() {
        let cmd = parse_command(&["METRICS"]).unwrap();
        assert!(matches!(cmd, CourierCommand::Metrics));
    }

    #[test]
    fn test_acl_public_commands() {
        let cmds = [
            CourierCommand::Health,
            CourierCommand::Ping,
            CourierCommand::CommandList,
            CourierCommand::Metrics,
            CourierCommand::ChannelList,
        ];
        for cmd in &cmds {
            assert_eq!(cmd.acl_requirement(), AclRequirement::None);
        }
    }

    #[test]
    fn test_acl_admin_commands() {
        let cmds: Vec<CourierCommand> = vec![
            CourierCommand::ChannelCreate {
                name: "x".into(),
                channel_type: "email".into(),
                config_json: "{}".into(),
            },
            CourierCommand::ChannelDelete { name: "x".into() },
            CourierCommand::DeliveryGet { id: "x".into() },
            CourierCommand::DeliveryList {
                channel: None,
                limit: 100,
            },
        ];
        for cmd in &cmds {
            assert_eq!(cmd.acl_requirement(), AclRequirement::Admin);
        }
    }

    #[test]
    fn test_acl_namespace_commands() {
        let cmd = CourierCommand::ChannelGet {
            name: "email-prod".into(),
        };
        match cmd.acl_requirement() {
            AclRequirement::Namespace { ns, scope, .. } => {
                assert_eq!(ns, "courier.email-prod.*");
                assert_eq!(scope, Scope::Read);
            }
            _ => panic!("expected Namespace requirement"),
        }
    }
}
