//! Low-level websocket types and methods.

pub mod error;
pub mod payload;

pub use error::Error;

use super::rtp::{self, Encryptor, Socket};
use error::{ApiError, ProtocolError};
use payload::{
    ClientConnect, ClientDisconnect, EncryptionMode, GatewayEvent, Heartbeat, Hello, Identify,
    Ready, Resume, SelectProtocol, SelectProtocolData, SessionDescription, Speaking,
};

use tokio::net::UdpSocket;
use tokio::time::{sleep_until, Duration, Instant};

use async_tungstenite::{
    tokio::{connect_async, ConnectStream},
    WebSocketStream,
};
use futures_util::{Sink, SinkExt, Stream, StreamExt};
use serde::de::DeserializeSeed as _;
use tungstenite::protocol::{CloseFrame, Message};
use twilight_model::id::{
    marker::{GuildMarker, UserMarker},
    Id,
};

use std::borrow::Cow;
use std::fmt::Debug;

use tracing::{debug, debug_span, error, info, instrument, warn};

/// Unmanaged voice connection to a websocket.
///
/// This must be polled constantly to ensure heartbeats are sent. To poll the
/// connection, call [`Connection::recv`].
pub struct Connection {
    session: Session,
    wss: WebSocketStream<ConnectStream>,
    heartbeater: Heartbeater,
}

impl Connection {
    /// Establishes a connection to an endpoint.
    ///
    /// Returns the websocket connection and the UDP connection used to send
    /// Opus frames.
    #[instrument]
    pub async fn connect(session: Session) -> Result<(Connection, Socket), Error> {
        let (wss, _response) = connect_async(format!("wss://{}/?v=4", session.endpoint)).await?;

        let mut conn = Connection {
            session,
            wss,
            heartbeater: Default::default(),
        };
        let rtp = conn.handshake().await?;

        Ok((conn, rtp))
    }

    /// Polls for the next event.
    ///
    /// This is (should be) cancel-safe.
    #[instrument(skip(self))]
    pub async fn recv(&mut self) -> Option<Result<Event, Error>> {
        loop {
            tokio::select! {
                // next event
                ev = recv(&mut self.wss) => {
                    match ev {
                        Some(Ok(GatewayEvent::HeartbeatAck(ack))) => {
                            if self.heartbeater.nonce() == ack.0 {
                                debug!("voice heartbeat ACK");
                            } else {
                                warn!(nonce = ack.0, "invalid ACK");
                            }
                        }
                        Some(Ok(GatewayEvent::Speaking(ev))) => {
                            return Some(Ok(Event::Speaking(ev)));
                        }
                        Some(Ok(GatewayEvent::ClientConnect(ev))) => {
                            return Some(Ok(Event::ClientConnect(ev)));
                        }
                        Some(Ok(GatewayEvent::ClientDisconnect(ev))) => {
                            return Some(Ok(Event::ClientDisconnect(ev)));
                        }
                        Some(Ok(ev)) => {
                            warn!(?ev, "skipping ev");
                        }
                        Some(Err(Error::Protocol(err))) => {
                            warn!(%err, "ignoring protocol error");
                        }
                        Some(Err(err)) if err.can_resume() => {
                            match self.resume().await {
                                Ok(()) => (),
                                Err(err) => return Some(Err(err)),
                            }
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

    /// Sends an event to the remote endpoint.
    #[instrument(skip(self))]
    pub async fn send(&mut self, command: impl Command + Debug) -> Result<(), Error> {
        let ev = command.to_event();

        match send(&mut self.wss, &ev).await {
            Ok(()) => Ok(()),
            Err(err) if err.can_resume() => match self.resume().await {
                Ok(()) => send(&mut self.wss, &ev).await,
                Err(err) => return Err(err),
            },
            Err(err) => Err(err),
        }
    }

    /// Gets session information.
    pub fn session(&self) -> &Session {
        &self.session
    }

    /// Completes a handshake with the voice server. See [discord's docs][1]
    /// for more information.
    ///
    /// [1]: https://discord.com/developers/docs/topics/voice-connections#establishing-a-voice-websocket-connection
    #[instrument(name = "voice_handshake", skip(self))]
    async fn handshake(&mut self) -> Result<Socket, Error> {
        debug!(?self.session, "setting up connection");

        send(
            &mut self.wss,
            &GatewayEvent::Identify(Identify {
                guild_id: self.session.guild_id,
                user_id: self.session.user_id,
                session_id: self.session.session_id.clone(),
                token: self.session.token.clone(),
            }),
        )
        .await?;

        // wait for hello and ready events
        let span = debug_span!("waiting for Hello and Ready");
        let _span = span.enter();

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
                    warn!(?ev, "unexpected event");
                }
                Err(Error::Protocol(err)) => {
                    warn!(%err, "ignoring protocol error");
                }
                Err(err) => {
                    error!(%err, "ws error");
                    return Err(err);
                }
            }
        }

        let hello = hello.unwrap();
        let ready = ready.unwrap();

        drop(_span);

        // create heartbeater
        self.heartbeater = Heartbeater::new(hello.heartbeat_interval);

        // establish udp connection and discover ip
        let udp = UdpSocket::bind(("0.0.0.0", ready.port)).await?;
        udp.connect((ready.ip, ready.port)).await?;

        let ip = rtp::ip_discovery(&udp, ready.ssrc).await?;

        let span = debug_span!("select protocol");
        let _span = span.enter();

        // choose encryption mode
        // order: lite > suffix > normal
        let mode = ready
            .modes
            .iter()
            .find(|&m| *m == EncryptionMode::Lite)
            .or_else(|| ready.modes.iter().find(|&m| *m == EncryptionMode::Suffix))
            .or_else(|| ready.modes.iter().find(|&m| *m == EncryptionMode::Normal))
            .cloned()
            .unwrap();

        let encryptor_mode = match mode {
            EncryptionMode::Normal => rtp::EncryptionMode::Normal,
            EncryptionMode::Suffix => rtp::EncryptionMode::Suffix,
            EncryptionMode::Lite => rtp::EncryptionMode::Lite,
            mode => {
                return Err(Error::Protocol(ProtocolError::UnsupportedEncryptionMode(
                    mode,
                )))
            }
        };

        debug!(%mode, "selected encryption mode");

        // select protocol
        send(
            &mut self.wss,
            &GatewayEvent::SelectProtocol(SelectProtocol {
                protocol: String::from("udp"),
                data: SelectProtocolData {
                    address: format!("{}", ip.ip()),
                    port: ip.port(),
                    mode,
                },
            }),
        )
        .await?;

        drop(_span);

        // wait for response
        let span = debug_span!("wait for response");
        let _span = span.enter();

        let mut desc: Option<SessionDescription> = None;

        while let Some(ev) = recv(&mut self.wss).await {
            match ev {
                Ok(GatewayEvent::SessionDescription(ev)) => {
                    desc = Some(ev);
                    break;
                }
                Ok(ev) => {
                    warn!(?ev, "unexpected event");
                }
                Err(Error::Protocol(err)) => {
                    warn!(%err, "ignoring protocol error");
                }
                Err(err) => {
                    error!(%err, "ws error");
                    return Err(err);
                }
            }
        }

        let desc = desc.unwrap();

        info!(endpoint = self.session.endpoint, "voice connected");

        Ok(Socket::new(
            udp,
            ready.ssrc,
            Encryptor::new(encryptor_mode, desc.secret_key),
        ))
    }

    /// Completes a session resume handshake with the voice server. See
    /// [discord's docs][1] for more information.
    ///
    /// [1]: https://discord.com/developers/docs/topics/voice-connections#establishing-a-voice-websocket-connection
    #[instrument(name = "voice_resume", skip(self))]
    async fn resume(&mut self) -> Result<(), Error> {
        let (wss, _response) =
            connect_async(format!("wss://{}/?v=4", self.session.endpoint)).await?;

        debug!("got new connection");
        self.wss = wss;

        send(
            &mut self.wss,
            &GatewayEvent::Resume(Resume {
                guild_id: self.session.guild_id,
                session_id: self.session.session_id.clone(),
                token: self.session.token.clone(),
            }),
        )
        .await?;

        // wait for response
        let span = debug_span!("wait for resume response");
        let _span = span.enter();

        while let Some(ev) = recv(&mut self.wss).await {
            match ev {
                Ok(GatewayEvent::Resumed) => {
                    break;
                }
                Ok(ev) => {
                    warn!(?ev, "unexpected event");
                }
                Err(err) => {
                    error!(%err, "resumed failed");
                    return Err(err);
                }
            }
        }

        Ok(())
    }

    /// Disconnects gracefully from the gateway.
    ///
    /// The websocket should not be used after this.
    ///
    /// # Panics
    /// Panics if closing the socket failed.
    pub async fn disconnect(&mut self) {
        self.wss
            .close(Some(CloseFrame {
                code: tungstenite::protocol::frame::coding::CloseCode::Normal,
                reason: Cow::Borrowed("Disconnected from gateway"),
            }))
            .await
            .unwrap();
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
                                Err(err) => {
                                    return Some(Err(Error::Protocol(ProtocolError::Deser(
                                        err, msg,
                                    ))))
                                }
                            }
                        }
                        None => {
                            return Some(Err(Error::Protocol(ProtocolError::MissingOpcode)));
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
    let msg = serde_json::to_string(ev).map_err(|e| Error::Protocol(ProtocolError::Ser(e)))?;

    // send message
    wss.send(Message::Text(msg)).await?;

    Ok(())
}

/// Session information of a websocket.
#[derive(Debug)]
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
    ClientConnect(ClientConnect),
    ClientDisconnect(ClientDisconnect),
}

/// Voice command.
pub trait Command {
    fn to_event(self) -> GatewayEvent;
}

impl Command for Speaking {
    fn to_event(self) -> GatewayEvent {
        GatewayEvent::Speaking(self)
    }
}

/// Manages heartbeat state.
#[derive(Debug)]
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
