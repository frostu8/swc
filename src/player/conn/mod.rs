//! Voice websocket things.

pub mod error;
pub mod payload;

use error::{ApiError, Error};
use payload::{GatewayEvent, Speaking, ClientDisconnect, Heartbeat, Hello, Ready, Identify};

use async_tungstenite::{WebSocketStream, tokio::{connect_async, ConnectStream}};
use tungstenite::protocol::Message;
use twilight_model::id::{Id, marker::{GuildMarker, UserMarker}};
use futures_util::{Stream, StreamExt, Sink, SinkExt};
use tokio::time::{Instant, Duration, sleep_until};
use serde::de::DeserializeSeed as _;

/// Unmanaged voice connection to a websocket.
///
/// This must be polled constantly to ensure heartbeats are sent. To poll the
/// connection, call [`Connection::next`].
pub struct Connection {
    session: Session,
    wss: WebSocketStream<ConnectStream>,
    heartbeater: Heartbeater,
}

impl Connection {
    /// Establishes a connection to an endpoint.
    pub async fn connect(session: Session) -> Result<Connection, Error> {
        let (wss, _response) = connect_async(format!("wss://{}/?v=4", session.endpoint)).await?;

        let mut conn = Connection { session, wss, heartbeater: Default::default() };
        conn.handshake().await?;

        Ok(conn)
    }

    /// Polls for the next event.
    ///
    /// This is (should be) cancel-safe.
    pub async fn next(&mut self) -> Option<Result<Event, Error>> {
        loop {
            tokio::select! {
                // next event
                ev = recv(&mut self.wss) => {
                    match ev {
                        Some(Ok(GatewayEvent::HeartbeatAck(ack))) => {
                            if self.heartbeater.nonce() == ack.0 {
                                debug!("voice heartbeat ACK");
                            } else {
                                warn!("invalid ACK, nonce: {}", ack.0);
                            }
                        }
                        Some(Ok(ev)) => {
                            todo!()
                        }
                        Some(Err(err)) => {
                            return Some(Err(err));
                        }
                        None => return None
                    }
                }
                // wait for heartbeats
                heartbeat = self.heartbeater.next() => {
                    // send heartbeat
                    send(&mut self.wss, &GatewayEvent::Heartbeat(heartbeat)).await.unwrap();
                }
            }
        }
    }

    /// Completes a handshake with the voice server. See [discord's docs][1] for
    /// more information.
    ///
    /// [1]: https://discord.com/developers/docs/topics/voice-connections#establishing-a-voice-websocket-connection
    async fn handshake(&mut self) -> Result<(), Error> {
        debug!("begin websocket handshake @ {}", self.session.endpoint);

        send(&mut self.wss, &GatewayEvent::Identify(Identify {
            guild_id: self.session.guild_id,
            user_id: self.session.user_id,
            session_id: self.session.session_id.clone(),
            token: self.session.token.clone(),
        }))
        .await?;

        debug!("waiting for response");

        // wait for hello and ready events
        let mut hello: Option<Hello> = None;
        let mut ready: Option<Ready> = None;

        while let Some(ev) = recv(&mut self.wss).await {
            match ev {
                Ok(GatewayEvent::Hello(ev)) => {
                    hello = Some(ev);

                    if hello.is_some() && ready.is_some() {
                        break;
                    }
                }
                Ok(GatewayEvent::Ready(ev)) => {
                    ready = Some(ev);

                    if hello.is_some() && ready.is_some() {
                        break;
                    }
                }
                Ok(ev) => {
                    warn!("unexpected event: {:?}", ev);
                }
                Err(err) => {
                    error!("ws error: {}", err);
                    return Err(err);
                }
            }
        }

        // create heartbeater
        self.heartbeater = Heartbeater::new(hello.unwrap().heartbeat_interval);

        Ok(())
    }
}

/// Receives a gateway event from the server.
async fn recv(
    mut wss: impl Stream<Item = Result<Message, tungstenite::error::Error>> + Unpin,
) -> Option<Result<GatewayEvent, Error>> {
    while let Some(res) = wss.next().await {
        match res {
            Ok(message) => match message {
                Message::Text(msg) => {
                    let event = payload::GatewayEventDeserializer::from_json(&msg);

                    match event {
                        Some(event) => {
                            let mut json = serde_json::Deserializer::from_str(&msg);
                            
                            match event.deserialize(&mut json) {
                                Ok(event) => return Some(Ok(event)),
                                Err(err) => return Some(Err(Error::Json(err))),
                            }
                        }
                        None => {
                            return Some(Err(Error::MissingOpcode));
                        }
                    }
                }
                Message::Close(Some(frame)) => {
                    if let Some(code) = error::Code::from_code(frame.code) {
                        return Some(Err(Error::Api(ApiError {
                            code,
                            message: frame.reason.into_owned(),
                        })));
                    } else {
                        return Some(Err(Error::Closed(Some(frame))));
                    }
                }
                Message::Close(None) => return Some(Err(Error::Closed(None))),
                // if a ping or pong event is received, silently drop
                _ => (),
            },
            Err(err) => return Some(Err(err.into())),
        }
    }

    // if we have reached the end of the stream, return `None`
    None
}

/// Sends a gateway event to the server.
async fn send(
    mut wss: impl Sink<Message, Error = tungstenite::error::Error> + Unpin,
    ev: &GatewayEvent,
) -> Result<(), Error> {
    // serialize event
    let msg = serde_json::to_string(ev).map_err(Error::Json)?;

    // send message
    wss.send(Message::Text(msg)).await?;

    Ok(())
}

/// Session information of a websocket.
pub struct Session {
    /// The endpoint of the session.
    pub endpoint: String,
    /// Id of the server of the voice channel.
    pub guild_id: Id<GuildMarker>,
    /// Id of the current user.
    pub user_id: Id<UserMarker>,
    /// The id of the session.
    pub session_id: String,
    /// The token of the session.
    pub token: String,
}

/// Voice event.
#[derive(Debug)]
pub enum Event {
    Speaking(Speaking),
    ClientDisconnect(ClientDisconnect),
}

/// Manages heartbeat state.
struct Heartbeater {
    interval: f32,
    nonce: u64,
    next: Instant,
}

impl Heartbeater {
    /// Creates a new heartbeater.
    pub fn new(interval: f32) -> Heartbeater {
        Heartbeater {
            interval,
            nonce: 0,
            next: Instant::now() + Duration::from_millis(interval as u64),
        }
    }

    /// Returns the next heartbeat after the alloted time has passed.
    pub async fn next(&mut self) -> Heartbeat {
        sleep_until(self.next).await;

        self.nonce += 1;
        let heartbeat = Heartbeat(self.nonce);
        self.next = Instant::now() + Duration::from_millis(self.interval as u64);

        heartbeat
    }

    /// The current nonce of the heartbeater.
    pub fn nonce(&self) -> u64 {
        self.nonce
    }
}

impl Default for Heartbeater {
    fn default() -> Heartbeater {
        Heartbeater::new(15.0)
    }
}

