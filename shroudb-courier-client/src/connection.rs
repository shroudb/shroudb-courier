use crate::error::ClientError;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

pub struct Connection {
    reader: BufReader<tokio::net::tcp::OwnedReadHalf>,
    writer: tokio::net::tcp::OwnedWriteHalf,
}

impl Connection {
    pub async fn connect(addr: &str) -> Result<Self, ClientError> {
        let stream = TcpStream::connect(addr)
            .await
            .map_err(|e| ClientError::Connection(e.to_string()))?;
        let (reader, writer) = stream.into_split();
        Ok(Self {
            reader: BufReader::new(reader),
            writer,
        })
    }

    pub async fn send_command(&mut self, args: &[&str]) -> Result<Value, ClientError> {
        // Encode RESP3 array of bulk strings
        let mut buf = format!("*{}\r\n", args.len());
        for arg in args {
            buf.push_str(&format!("${}\r\n{}\r\n", arg.len(), arg));
        }

        self.writer
            .write_all(buf.as_bytes())
            .await
            .map_err(|e| ClientError::Connection(e.to_string()))?;
        self.writer
            .flush()
            .await
            .map_err(|e| ClientError::Connection(e.to_string()))?;

        self.read_response().await
    }

    async fn read_response(&mut self) -> Result<Value, ClientError> {
        let mut line = String::new();
        self.reader
            .read_line(&mut line)
            .await
            .map_err(|e| ClientError::Connection(e.to_string()))?;

        if line.is_empty() {
            return Err(ClientError::Connection("connection closed".into()));
        }

        let line = line.trim_end();

        match line.as_bytes().first() {
            Some(b'+') => {
                // Simple string
                let s = &line[1..];
                Ok(Value::String(s.to_string()))
            }
            Some(b'-') => {
                // Error
                let msg = line[1..].strip_prefix("ERR ").unwrap_or(&line[1..]);
                Err(ClientError::Server(msg.to_string()))
            }
            Some(b'$') => {
                // Bulk string
                let len: usize = line[1..]
                    .parse()
                    .map_err(|e| ClientError::Protocol(format!("invalid bulk length: {e}")))?;

                let mut data = vec![0u8; len + 2]; // +2 for \r\n
                tokio::io::AsyncReadExt::read_exact(&mut self.reader, &mut data)
                    .await
                    .map_err(|e| ClientError::Connection(e.to_string()))?;

                let body = &data[..len];
                let value: Value = serde_json::from_slice(body)
                    .map_err(|e| ClientError::Protocol(format!("invalid JSON: {e}")))?;
                Ok(value)
            }
            Some(b) => Err(ClientError::Protocol(format!(
                "unexpected response type: {}",
                *b as char
            ))),
            None => Err(ClientError::Connection("empty response".into())),
        }
    }
}
