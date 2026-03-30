pub mod channel;
pub mod delivery;
pub mod error;

pub use channel::{Channel, ChannelType, SmtpConfig, WebhookConfig};
pub use delivery::{
    ContentType, DeliveryReceipt, DeliveryRequest, DeliveryStatus, RenderedMessage,
};
pub use error::CourierError;
