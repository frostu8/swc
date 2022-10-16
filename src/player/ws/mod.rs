//! Voice websocket things.

pub mod error;
pub mod payload;

use async_tungstenite::{WebSocketStream, tokio::{connect_async, ConnectStream}};
use tungstenite::protocol::Message;
use futures_util::{StreamExt, SinkExt};
use error::{ApiError, Error};
use serde::de::DeserializeSeed as _;
use payload::Event;

/// Stateless websocket.
pub struct WebSocket(WebSocketStream<ConnectStream>);

impl WebSocket {
    /// Establishes a connection to an endpoint.
    pub async fn new(endpoint: &str) -> Result<WebSocket, Error> {
        let (wss, _response) = connect_async(format!("wss://{}/?v=4", endpoint)).await?;

        Ok(WebSocket(wss))
    }

    /// Receives an event from the server.
    pub async fn recv(&mut self) -> Result<Option<Event>, Error> {
        loop {
            match self.0.next().await {
                Some(Ok(message)) => match message {
                    Message::Text(msg) => {
                        let event = payload::EventDeserializer::from_json(&msg);

                        match event {
                            Some(event) => {
                                let mut json = serde_json::Deserializer::from_str(&msg);
                                
                                match event.deserialize(&mut json) {
                                    Ok(event) => return Ok(Some(event)),
                                    Err(err) => return Err(Error::Json(err)),
                                }
                            }
                            None => {
                                return Err(Error::MissingOpcode);
                            }
                        }
                    }
                    Message::Close(Some(frame)) => {
                        if let Some(code) = error::Code::from_code(frame.code) {
                            return Err(Error::Api(ApiError {
                                code,
                                message: frame.reason.into_owned(),
                            }));
                        } else {
                            return Err(Error::Closed(Some(frame)));
                        }
                    }
                    Message::Close(None) => return Err(Error::Closed(None)),
                    // if a ping or pong event is received, silently drop
                    _ => (),
                },
                Some(Err(err)) => return Err(Error::Ws(err)),
                None => return Ok(None),
            }
        }
    }

    /// Sends an event to the server.
    pub async fn send(&mut self, ev: &Event) -> Result<(), Error> {
        // serialize event
        let msg = serde_json::to_string(ev).map_err(Error::Json)?;

        // send message
        self.0.send(Message::Text(msg)).await?;

        Ok(())
    }
}

