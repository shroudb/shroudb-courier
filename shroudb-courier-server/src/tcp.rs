use shroudb_acl::{AclRequirement, AuthContext, TokenValidator};
use shroudb_courier_engine::CourierEngine;
use shroudb_courier_protocol::{CourierResponse, dispatch, parse_command};
use shroudb_protocol_wire::Resp3Frame;
use shroudb_protocol_wire::reader::read_frame;
use shroudb_protocol_wire::writer::write_frame;
use shroudb_store::Store;
use std::sync::Arc;
use tokio::io::BufReader;
use tokio::net::TcpListener;
use tokio::sync::watch;

pub async fn run_tcp<S: Store + 'static>(
    listener: TcpListener,
    engine: Arc<CourierEngine<S>>,
    token_validator: Option<Arc<dyn TokenValidator>>,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    loop {
        tokio::select! {
            result = listener.accept() => {
                match result {
                    Ok((stream, addr)) => {
                        tracing::debug!(%addr, "accepted connection");
                        let engine = Arc::clone(&engine);
                        let tv = token_validator.clone();
                        tokio::spawn(async move {
                            if let Err(e) = handle_connection(stream, &engine, tv.as_deref()).await {
                                tracing::debug!(%addr, error = %e, "connection closed");
                            }
                        });
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "accept error");
                    }
                }
            }
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    tracing::info!("TCP server shutting down");
                    break;
                }
            }
        }
    }
}

async fn handle_connection<S: Store>(
    stream: tokio::net::TcpStream,
    engine: &CourierEngine<S>,
    token_validator: Option<&dyn TokenValidator>,
) -> Result<(), Box<dyn std::error::Error>> {
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    let mut auth_context: Option<AuthContext> = None;
    let auth_required = token_validator.is_some();

    loop {
        let frame = match read_frame(&mut reader).await? {
            Some(f) => f,
            None => return Ok(()), // clean EOF
        };

        let args = match frame_to_args(&frame) {
            Ok(a) => a,
            Err(e) => {
                let resp = CourierResponse::error(e);
                write_frame(&mut writer, &response_to_frame(&resp)).await?;
                continue;
            }
        };

        let arg_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        let cmd = match parse_command(&arg_refs) {
            Ok(c) => c,
            Err(e) => {
                let resp = CourierResponse::error(e);
                write_frame(&mut writer, &response_to_frame(&resp)).await?;
                continue;
            }
        };

        // Handle AUTH at connection layer
        if let shroudb_courier_protocol::CourierCommand::Auth { token } = &cmd {
            match token_validator {
                Some(tv) => match tv.validate(token) {
                    Ok(t) => {
                        auth_context = Some(t.into_context());
                        let resp = CourierResponse::ok(serde_json::json!({"status": "ok"}));
                        write_frame(&mut writer, &response_to_frame(&resp)).await?;
                        continue;
                    }
                    Err(e) => {
                        let resp = CourierResponse::error(format!("auth failed: {e}"));
                        write_frame(&mut writer, &response_to_frame(&resp)).await?;
                        continue;
                    }
                },
                None => {
                    let resp = CourierResponse::ok(serde_json::json!({"status": "ok"}));
                    write_frame(&mut writer, &response_to_frame(&resp)).await?;
                    continue;
                }
            }
        }

        // Reject non-public commands when auth required but not authenticated
        if auth_required && auth_context.is_none() {
            let requirement = cmd.acl_requirement();
            if requirement != AclRequirement::None {
                let resp = CourierResponse::error("authentication required");
                write_frame(&mut writer, &response_to_frame(&resp)).await?;
                continue;
            }
        }

        let response = dispatch(engine, cmd, auth_context.as_ref()).await;
        write_frame(&mut writer, &response_to_frame(&response)).await?;
    }
}

fn frame_to_args(frame: &Resp3Frame) -> Result<Vec<String>, String> {
    match frame {
        Resp3Frame::Array(items) => {
            let mut args = Vec::with_capacity(items.len());
            for item in items {
                match item {
                    Resp3Frame::BulkString(b) => {
                        args.push(
                            String::from_utf8(b.clone())
                                .map_err(|e| format!("invalid UTF-8: {e}"))?,
                        );
                    }
                    Resp3Frame::SimpleString(s) => {
                        args.push(s.clone());
                    }
                    other => {
                        return Err(format!("unexpected frame type in command array: {other:?}"));
                    }
                }
            }
            Ok(args)
        }
        other => Err(format!("expected array, got: {other:?}")),
    }
}

fn response_to_frame(response: &CourierResponse) -> Resp3Frame {
    match response {
        CourierResponse::Ok(value) => {
            let json = serde_json::to_string(value).unwrap_or_else(|_| "{}".into());
            Resp3Frame::BulkString(json.into_bytes())
        }
        CourierResponse::Error(msg) => Resp3Frame::SimpleError(format!("ERR {msg}")),
    }
}
