use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;
use std::sync::Arc;

use shroudb_acl::{AclRequirement, AuthContext, TokenValidator};
use shroudb_courier_engine::CourierEngine;
use shroudb_courier_protocol::commands::{CourierCommand, parse_command};
use shroudb_courier_protocol::dispatch::dispatch;
use shroudb_courier_protocol::response::CourierResponse;
use shroudb_protocol_wire::Resp3Frame;
use shroudb_server_tcp::ServerProtocol;
use shroudb_store::Store;

pub struct CourierProtocol<S>(PhantomData<S>);

impl<S: Store + 'static> ServerProtocol for CourierProtocol<S> {
    type Command = CourierCommand;
    type Response = CourierResponse;
    type Engine = CourierEngine<S>;

    fn engine_name(&self) -> &str {
        "courier"
    }

    fn parse_command(&self, args: &[&str]) -> Result<Self::Command, String> {
        parse_command(args)
    }

    fn auth_token(cmd: &Self::Command) -> Option<&str> {
        if let CourierCommand::Auth { token } = cmd {
            Some(token)
        } else {
            None
        }
    }

    fn acl_requirement(cmd: &Self::Command) -> AclRequirement {
        cmd.acl_requirement()
    }

    fn dispatch<'a>(
        &'a self,
        engine: &'a Self::Engine,
        cmd: Self::Command,
        auth: Option<&'a AuthContext>,
    ) -> Pin<Box<dyn Future<Output = Self::Response> + Send + 'a>> {
        Box::pin(dispatch(engine, cmd, auth))
    }

    fn response_to_frame(&self, response: &Self::Response) -> Resp3Frame {
        match response {
            CourierResponse::Ok(value) => {
                let json = serde_json::to_string(value).unwrap_or_else(|_| "{}".into());
                Resp3Frame::BulkString(json.into_bytes())
            }
            CourierResponse::Error(msg) => Resp3Frame::SimpleError(format!("ERR {msg}")),
        }
    }

    fn error_response(&self, msg: String) -> Self::Response {
        CourierResponse::error(msg)
    }

    fn ok_response(&self) -> Self::Response {
        CourierResponse::ok_simple()
    }
}

pub async fn run_tcp<S: Store + 'static>(
    listener: tokio::net::TcpListener,
    engine: Arc<CourierEngine<S>>,
    token_validator: Option<Arc<dyn TokenValidator>>,
    shutdown_rx: tokio::sync::watch::Receiver<bool>,
    tls_acceptor: Option<tokio_rustls::TlsAcceptor>,
) {
    shroudb_server_tcp::run_tcp_tls(
        listener,
        engine,
        Arc::new(CourierProtocol::<S>(PhantomData)),
        token_validator,
        shutdown_rx,
        tls_acceptor,
    )
    .await;
}
