//! Provides the audio [`Player`] and the player manager [`Manager`].

mod conn;
mod manager;
pub mod source;

pub use manager::Manager;

use conn::{payload::Speaking, Connection, Event, RtpPacket, Session};
use source::Source;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::time::{sleep_until, timeout_at, Duration, Instant};
use twilight_model::{
    gateway::payload::incoming::{VoiceServerUpdate, VoiceStateUpdate},
    id::{
        marker::{GuildMarker, UserMarker},
        Id,
    },
};

/// A music player for a specific guild.
#[derive(Clone)]
pub struct Player {
    guild_id: Id<GuildMarker>,
    gateway_tx: UnboundedSender<GatewayEvent>,
}

impl Player {
    /// Creates a new player.
    pub async fn new(
        user_id: impl Into<Id<UserMarker>>,
        guild_id: impl Into<Id<GuildMarker>>,
    ) -> Player {
        let user_id = user_id.into();
        let guild_id = guild_id.into();

        let (gateway_tx, gateway_rx) = mpsc::unbounded_channel();

        // spawn player task
        tokio::spawn(run(guild_id, user_id, gateway_rx));

        // create Player
        Player {
            gateway_tx,
            guild_id,
        }
    }

    /// The guild id of the player.
    pub fn guild_id(&self) -> Id<GuildMarker> {
        self.guild_id
    }

    /// Check if the player is closed.
    pub fn is_closed(&self) -> bool {
        self.gateway_tx.is_closed()
    }

    /// Updates the player's state with a [`VoiceStateUpdate`] event.
    pub fn voice_state_update(&self, ev: Box<VoiceStateUpdate>) -> Result<(), Error> {
        self.gateway_tx
            .send(GatewayEvent::VoiceStateUpdate(ev))
            .map_err(|_| Error)
    }

    /// Updates the player's state with a [`VoiceServerUpdate`] event.
    pub fn voice_server_update(&self, ev: VoiceServerUpdate) -> Result<(), Error> {
        self.gateway_tx
            .send(GatewayEvent::VoiceServerUpdate(ev))
            .map_err(|_| Error)
    }
}

/// An error for operations with [`Player`].
///
/// A [`Player`] may stop at any time in response to an unrecoverable error.
/// Things like being disconnected administratively by someone who has that
/// power is an unrecoverable error, and so is Discord servers going entirely
/// down.
#[derive(Clone, Copy, Debug)]
pub struct Error;

#[derive(Debug)]
enum GatewayEvent {
    VoiceStateUpdate(Box<VoiceStateUpdate>),
    VoiceServerUpdate(VoiceServerUpdate),
}

async fn run(
    guild_id: Id<GuildMarker>,
    user_id: Id<UserMarker>,
    mut gateway: UnboundedReceiver<GatewayEvent>,
) {
    // begin initialization: wait for events
    let timeout = Instant::now() + Duration::from_millis(5000);
    let mut vstu: Option<Box<VoiceStateUpdate>> = None;
    let mut vseu: Option<VoiceServerUpdate> = None;

    debug!("waiting for VoiceStateUpdate and VoiceServerUpdate...");

    loop {
        tokio::select! {
            _ = sleep_until(timeout) => {
                error!("player initialization timed out after 5000ms!");
                return;
            }
            Some(ev) = gateway.recv() => {
                match ev {
                    GatewayEvent::VoiceStateUpdate(ev) if ev.0.user_id == user_id => {
                        vstu = Some(ev);
                    },
                    GatewayEvent::VoiceServerUpdate(ev) => {
                        vseu = Some(ev);
                    },
                    _ => ()
                };

                if vstu.is_some() && vseu.is_some() {
                    break;
                }
            }
        }
    }

    // establish session
    let session = if let (Some(vseu), Some(vstu)) = (vseu, vstu) {
        Session {
            guild_id,
            user_id,
            endpoint: vseu.endpoint.unwrap(),
            token: vseu.token,
            session_id: vstu.0.session_id,
        }
    } else {
        error!("got no response for voice state update");
        return;
    };

    debug!("got new session; establishing ws connection...");

    let timeout = Instant::now() + Duration::from_millis(5000);
    let (mut conn, mut rtp) = match timeout_at(timeout, Connection::connect(session)).await {
        Ok(Ok(conn)) => conn,
        Ok(Err(err)) => {
            error!("connect: {}", err);
            return;
        }
        Err(_) => {
            error!("player initialization timed out after 5000ms!");
            return;
        }
    };

    // TODO: testing
    let mut source = Source::new("https://www.youtube.com/watch?v=7O-TZdXh7Y4")
        .await
        .unwrap();
    let mut packet = RtpPacket::default();

    let mut next_burst = Instant::now();
    let mut burst = 0;
    let burst_amount = 5;

    conn.send(Speaking {
        speaking: 1,
        ssrc: rtp.ssrc(),
        delay: Some(0),
    })
    .await
    .unwrap();

    loop {
        tokio::select! {
            biased;

            // voice websocket event
            ev = conn.recv() => {
                match ev {
                    Some(Ok(Event::ChangeSocket(new_rtp))) => {
                        // update socket
                        rtp = new_rtp;
                    }
                    Some(Ok(ev)) => {
                        // discard event
                        debug!("voice ev: {:?}", ev);
                    }
                    Some(Err(err)) if err.disconnected() => {
                        // normal disconnect event
                        debug!("disconnected");
                        break;
                    }
                    Some(Err(err)) => {
                        // print error and continue
                        error!("voice: {}", err);
                        break;
                    }
                    None => break
                }
            }
            // main gateway event
            ev = gateway.recv() => {
                match ev {
                    Some(GatewayEvent::VoiceServerUpdate(vseu)) => {
                        // server update; reconnect
                        let session = Session {
                            guild_id, user_id,
                            endpoint: vseu.endpoint.unwrap(),
                            token: vseu.token,
                            session_id: conn.session().session_id.clone(),
                        };
                        let timeout = Instant::now() + Duration::from_millis(5000);
                        (conn, rtp) = match timeout_at(timeout, Connection::connect(session)).await {
                            Ok(Ok(conn)) => conn,
                            Ok(Err(err)) => {
                                error!("connect: {}", err);
                                break;
                            }
                            Err(_) => {
                                error!("player reconnection timed out after 5000ms!");
                                break;
                            }
                        };
                    }
                    Some(GatewayEvent::VoiceStateUpdate(_vstu)) => {
                        // TODO: handle state update
                    }
                    None => {
                        error!("gateway closed unexpectedly");
                        break;
                    }
                }
            }
            _ = sleep_until(next_burst) => {
                burst = 5;
                next_burst = Instant::now() + crate::constants::TIMESTEP_LENGTH * burst_amount;
            }
            // completion of packet reading event
            result = source.read(packet.payload_mut()), if burst > 0 => {
                match result {
                    Ok(len) => {
                        // encrypt and send packet
                        if len > 0 {
                            packet.set_payload_len(len);
                            match rtp.send(packet).await {
                                Ok(()) => (),
                                Err(err) => {
                                    error!("packet: {}", err);
                                    break;
                                }
                            }
                            packet = RtpPacket::default();
                        }

                        burst -= 1;
                    }
                    Err(err) => {
                        error!("audio read: {}", err);
                        break;
                    }
                }
            }
        }
    }

    // kill source gracefully
    let _ = source.kill().await;
}

async fn until<F>(deadline: Instant, fut: F) -> F::Output
where
    F: std::future::Future,
{
    sleep_until(deadline).await;
    fut.await
}
