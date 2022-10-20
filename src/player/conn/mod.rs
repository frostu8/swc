//! Voice connection things.

pub mod error;
pub mod payload;
pub mod rtp;

pub use error::Error;
pub use rtp::{Socket as RtpSocket, Packet as RtpPacket};

use error::ApiError;
use payload::{
    GatewayEvent, Speaking, ClientDisconnect, Heartbeat, Hello, Ready, Identify,
    SelectProtocol, SelectProtocolData, SessionDescription, Resume,
    EncryptionMode,
};
use rtp::Encryptor;

use tokio::time::{Instant, Duration, sleep_until};
use tokio::net::UdpSocket;

use async_tungstenite::{WebSocketStream, tokio::{connect_async, ConnectStream}};
use tungstenite::protocol::Message;
use twilight_model::id::{Id, marker::{GuildMarker, UserMarker}};
use futures_util::{Stream, StreamExt, Sink, SinkExt};
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
    pub async fn connect(session: Session) -> Result<(Connection, RtpSocket), Error> {
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
                                warn!("invalid ACK, nonce: {}", ack.0);
                            }
                        }
                        Some(Ok(GatewayEvent::Speaking(ev))) => {
                            return Some(Ok(Event::Speaking(ev)));
                        }
                        Some(Ok(GatewayEvent::ClientDisconnect(ev))) => {
                            return Some(Ok(Event::ClientDisconnect(ev)));
                        }
                        Some(Ok(ev)) => {
                            warn!("skipping ev: {:?}", ev);
                        }
                        Some(Err(err)) if err.can_resume() => {
                            match self.resume().await {
                                Ok(Some(rtp)) => return Some(Ok(Event::ChangeSocket(rtp))),
                                Ok(None) => (),
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
    pub async fn send(&mut self, command: impl Command) -> Result<(), Error> {
        send(&mut self.wss, &command.to_event())
            .await
            .map_err(Into::into)
    }
    
    /// Gets session information.
    pub fn session(&self) -> &Session {
        &self.session
    }

    /// Completes a handshake with the voice server. See [discord's docs][1] for
    /// more information.
    ///
    /// [1]: https://discord.com/developers/docs/topics/voice-connections#establishing-a-voice-websocket-connection
    async fn handshake(&mut self) -> Result<RtpSocket, Error> {
        debug!("begin websocket handshake");

        send(&mut self.wss, &GatewayEvent::Identify(Identify {
            guild_id: self.session.guild_id,
            user_id: self.session.user_id,
            session_id: self.session.session_id.clone(),
            token: self.session.token.clone(),
        }))
        .await?;

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

        let hello = hello.unwrap();
        let ready = ready.unwrap();

        // create heartbeater
        self.heartbeater = Heartbeater::new(hello.heartbeat_interval);

        // establish udp connection and discover ip
        let udp = UdpSocket::bind(("0.0.0.0", ready.port)).await?;
        udp.connect((ready.ip, ready.port)).await?;

        let ip = rtp::ip_discovery(&udp, ready.ssrc).await?;

        // choose encryption mode
        // order: lite > suffix > normal
        let mode = ready.modes.iter().find(|&m| *m == EncryptionMode::Lite)
            .or_else(|| ready.modes.iter().find(|&m| *m == EncryptionMode::Suffix))
            .or_else(|| ready.modes.iter().find(|&m| *m == EncryptionMode::Normal))
            .cloned()
            .unwrap();

        let encryptor_mode = match mode {
            EncryptionMode::Normal => rtp::EncryptionMode::Normal,
            EncryptionMode::Suffix => rtp::EncryptionMode::Suffix,
            EncryptionMode::Lite => rtp::EncryptionMode::Lite,
            mode => return Err(Error::UnsupportedEncryptionMode(mode)),
        };

        debug!("selected encryption mode {}", mode);

        // select protocol
        send(&mut self.wss, &GatewayEvent::SelectProtocol(SelectProtocol {
            protocol: String::from("udp"),
            data: SelectProtocolData {
                address: format!("{}", ip.ip()),
                port: ip.port(),
                mode,
            },
        }))
        .await?;

        // wait for response
        let mut desc: Option<SessionDescription> = None;

        while let Some(ev) = recv(&mut self.wss).await {
            match ev {
                Ok(GatewayEvent::SessionDescription(ev)) => {
                    desc = Some(ev);
                    break;
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

        let desc = desc.unwrap();

        info!("voice connected to {}", self.session.endpoint);

        Ok(RtpSocket::new(
            udp,
            ready.ssrc,
            Encryptor::new(encryptor_mode, desc.secret_key),
        ))
    }

    /// Completes a session resume handshake with the voice server. See
    /// [discord's docs][1] for more information. If a reconnect is required,
    /// also returns the new `RtpSocket`.
    ///
    /// [1]: https://discord.com/developers/docs/topics/voice-connections#establishing-a-voice-websocket-connection
    async fn resume(&mut self) -> Result<Option<RtpSocket>, Error> {
        debug!("begin websocket resume handshake");

        send(&mut self.wss, &GatewayEvent::Resume(Resume {
            guild_id: self.session.guild_id,
            session_id: self.session.session_id.clone(),
            token: self.session.token.clone(),
        }))
        .await?;

        // wait for response
        while let Some(ev) = recv(&mut self.wss).await {
            match ev {
                Ok(GatewayEvent::Resumed) => {
                    break;
                }
                Ok(ev) => {
                    warn!("unexpected event: {:?}", ev);
                }
                Err(Error::Closed(_)) | Err(Error::Api(_)) => {
                    warn!("resume failed, attempting to reconnect");
                    return Ok(Some(self.handshake().await?));
                }
                Err(err) => {
                    error!("ws error: {}", err);
                    return Err(err);
                }
            }
        }

        Ok(None)
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
    /// Voice connection was disconnected and managed to reform the connection.
    ChangeSocket(RtpSocket),
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

