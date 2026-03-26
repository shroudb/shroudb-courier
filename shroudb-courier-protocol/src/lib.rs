//! Protocol layer for ShrouDB Courier.
//!
//! Command parsing, dispatch, handler execution, and response serialization.
//! Courier is a stateless notification delivery pipeline.

pub mod auth;
pub mod command;
pub mod command_parser;
pub mod dispatch;
pub mod error;
pub mod handlers;
pub mod resp3;
pub mod response;
pub mod serialize;

pub use command::Command;
pub use dispatch::CommandDispatcher;
pub use error::CommandError;
pub use resp3::Resp3Frame;
pub use response::{CommandResponse, ResponseMap, ResponseValue};
