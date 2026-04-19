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
            | CourierCommand::Hello => AclRequirement::None,

            // Channel enumeration and delivery metrics are NOT public: channel
            // names leak delivery intent (e.g. `finance-alerts`,
            // `customer-pii-webhook`) and per-channel counters leak activity
            // patterns. Keep them on the same admin tier as the rest of the
            // management surface.
            CourierCommand::ChannelList | CourierCommand::Metrics => AclRequirement::Admin,

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

fn looks_like_json(s: &str) -> bool {
    s.trim_start().starts_with('{')
}

fn build_channel_config_json(kv_args: &[&str]) -> Result<String, String> {
    let mut map = serde_json::Map::new();
    let mut i = 0;
    while i < kv_args.len() {
        let key = kv_args[i].to_uppercase();
        let val = kv_args
            .get(i + 1)
            .ok_or_else(|| format!("CHANNEL CREATE option {key} requires a value"))?;
        match key.as_str() {
            "URL" => {
                map.insert(
                    "default_recipient".into(),
                    serde_json::Value::String((*val).to_string()),
                );
            }
            other => {
                return Err(format!("unknown CHANNEL CREATE option: {other}"));
            }
        }
        i += 2;
    }
    Ok(serde_json::Value::Object(map).to_string())
}

fn build_deliver_request_json(
    channel: &str,
    recipient: &str,
    kv_args: &[&str],
) -> Result<String, String> {
    let mut map = serde_json::Map::new();
    map.insert(
        "channel".into(),
        serde_json::Value::String(channel.to_string()),
    );
    map.insert(
        "recipient".into(),
        serde_json::Value::String(recipient.to_string()),
    );
    let mut i = 0;
    while i < kv_args.len() {
        let key = kv_args[i].to_uppercase();
        let val = kv_args
            .get(i + 1)
            .ok_or_else(|| format!("DELIVER option {key} requires a value"))?;
        let field = match key.as_str() {
            "SUBJECT" => "subject",
            "BODY" => "body",
            "CONTENT_TYPE" => "content_type",
            other => return Err(format!("unknown DELIVER option: {other}")),
        };
        let value = if field == "content_type" {
            serde_json::Value::String(val.to_lowercase())
        } else {
            serde_json::Value::String((*val).to_string())
        };
        map.insert(field.into(), value);
        i += 2;
    }
    Ok(serde_json::Value::Object(map).to_string())
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
                    if args.len() < 4 {
                        return Err(
                            "CHANNEL CREATE requires <name> <type> [<config_json> | keyword args]"
                                .into(),
                        );
                    }
                    let config_json = if args.len() == 5 && looks_like_json(args[4]) {
                        args[4].to_string()
                    } else {
                        build_channel_config_json(&args[4..])?
                    };
                    Ok(CourierCommand::ChannelCreate {
                        name: args[2].to_string(),
                        channel_type: args[3].to_string(),
                        config_json,
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
                return Err(
                    "DELIVER requires a JSON request or <channel> <recipient> [keyword args]"
                        .into(),
                );
            }
            let request_json = if looks_like_json(args[1]) {
                args[1].to_string()
            } else {
                if args.len() < 3 {
                    return Err(
                        "DELIVER requires <channel> <recipient> [SUBJECT s] [BODY b] [CONTENT_TYPE t]"
                            .into(),
                    );
                }
                build_deliver_request_json(args[1], args[2], &args[3..])?
            };
            Ok(CourierCommand::Deliver { request_json })
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
        ];
        for cmd in &cmds {
            assert_eq!(cmd.acl_requirement(), AclRequirement::None);
        }
    }

    /// F-courier-7 (LOW): `CHANNEL LIST` and `METRICS` were declared as
    /// `AclRequirement::None`, which means an unauthenticated caller
    /// could enumerate every channel on the server and read per-channel
    /// delivery counts. Channel names leak intent (`finance-alerts`,
    /// `customer-pii-webhook`, …) and metrics leak activity patterns;
    /// neither is appropriate for the public tier. Bring them under the
    /// same `Admin` gate the rest of the management surface uses.
    #[test]
    fn debt_7_channel_list_and_metrics_must_not_be_public() {
        assert_eq!(
            CourierCommand::ChannelList.acl_requirement(),
            AclRequirement::Admin,
            "CHANNEL LIST must require admin — unauthenticated channel \
             enumeration leaks delivery intent"
        );
        assert_eq!(
            CourierCommand::Metrics.acl_requirement(),
            AclRequirement::Admin,
            "METRICS must require admin — unauthenticated per-channel \
             counters leak activity patterns"
        );
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

    /// F-courier-8 (MED): `CHANNEL CREATE` and `DELIVER` historically only
    /// accepted a JSON-blob payload (an `SmtpConfig` / `WebhookConfig` object
    /// or a full `DeliveryRequest`). This was inconsistent with every other
    /// ShrouDB engine's wire surface — including courier's own `DELIVERY LIST`
    /// — which uses keyword args (`FIELD`, `MODE`, `LIMIT`, `CHANNEL`, etc.).
    /// It also blocked moat's `debt_6_courier_must_record_chronicle_events`
    /// integration test, whose commands (`URL …` on create, `SUBJECT …`/`BODY …`
    /// on deliver) failed at parse before reaching the engine. The parser must
    /// accept keyword-arg forms in addition to the legacy JSON-blob forms.
    #[test]
    fn debt_8_parse_command_must_accept_keyword_arg_forms() {
        // --- CHANNEL CREATE keyword form ---
        // `URL <default_recipient>` is the minimum keyword supported today;
        // SMTP_HOST/SMTP_PORT/etc are separate follow-ups.
        let cmd = parse_command(&[
            "CHANNEL",
            "CREATE",
            "debt-ch",
            "webhook",
            "URL",
            "https://example.com/hook",
        ])
        .expect("keyword-arg CHANNEL CREATE must parse");
        match cmd {
            CourierCommand::ChannelCreate {
                name,
                channel_type,
                config_json,
            } => {
                assert_eq!(name, "debt-ch");
                assert_eq!(channel_type, "webhook");
                let v: serde_json::Value =
                    serde_json::from_str(&config_json).expect("keyword form must emit valid JSON");
                assert_eq!(
                    v.get("default_recipient").and_then(|x| x.as_str()),
                    Some("https://example.com/hook"),
                    "URL keyword must populate default_recipient; got {config_json}"
                );
            }
            _ => panic!("expected ChannelCreate"),
        }

        // No keyword args at all: empty config is valid.
        let cmd = parse_command(&["CHANNEL", "CREATE", "bare", "webhook"])
            .expect("CHANNEL CREATE without optional keywords must parse");
        match cmd {
            CourierCommand::ChannelCreate { config_json, .. } => {
                let v: serde_json::Value =
                    serde_json::from_str(&config_json).expect("empty config must be valid JSON");
                assert!(v.is_object(), "expected object, got {config_json}");
                assert!(
                    v.as_object().unwrap().is_empty(),
                    "expected empty object, got {config_json}"
                );
            }
            _ => panic!("expected ChannelCreate"),
        }

        // --- DELIVER keyword form ---
        let cmd = parse_command(&[
            "DELIVER",
            "debt-ch",
            "https://example.com/hook",
            "SUBJECT",
            "s",
            "BODY",
            "b",
        ])
        .expect("keyword-arg DELIVER must parse");
        match cmd {
            CourierCommand::Deliver { request_json } => {
                let v: serde_json::Value =
                    serde_json::from_str(&request_json).expect("keyword form must emit valid JSON");
                assert_eq!(v.get("channel").and_then(|x| x.as_str()), Some("debt-ch"));
                assert_eq!(
                    v.get("recipient").and_then(|x| x.as_str()),
                    Some("https://example.com/hook")
                );
                assert_eq!(v.get("subject").and_then(|x| x.as_str()), Some("s"));
                assert_eq!(v.get("body").and_then(|x| x.as_str()), Some("b"));
            }
            _ => panic!("expected Deliver"),
        }

        // --- Legacy JSON-blob forms must still parse ---
        let cmd = parse_command(&[
            "CHANNEL",
            "CREATE",
            "legacy",
            "email",
            r#"{"host":"s.example","port":587,"from_address":"x@y","starttls":true}"#,
        ])
        .expect("JSON-blob CHANNEL CREATE must still parse");
        assert!(matches!(cmd, CourierCommand::ChannelCreate { .. }));

        let cmd = parse_command(&["DELIVER", r#"{"channel":"x","recipient":"y","body":"z"}"#])
            .expect("JSON-blob DELIVER must still parse");
        assert!(matches!(cmd, CourierCommand::Deliver { .. }));

        // --- Unknown keywords must error with a clear message ---
        let err = parse_command(&["CHANNEL", "CREATE", "x", "webhook", "BOGUS", "v"])
            .expect_err("unknown keyword must be rejected");
        assert!(
            err.to_uppercase().contains("BOGUS"),
            "error should name the offending keyword; got: {err}"
        );

        let err = parse_command(&["DELIVER", "ch", "rec", "BOGUS", "v"])
            .expect_err("unknown keyword must be rejected");
        assert!(
            err.to_uppercase().contains("BOGUS"),
            "error should name the offending keyword; got: {err}"
        );
    }
}
