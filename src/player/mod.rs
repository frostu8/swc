//! Provides the audio [`Player`].

mod conn;

use tokio::sync::{
    mpsc::{self, UnboundedSender, UnboundedReceiver},
};
use tokio::time::{sleep_until, timeout_at, Instant, Duration};
use twilight_model::{
    id::{Id, marker::{GuildMarker, UserMarker}},
    gateway::payload::incoming::{VoiceStateUpdate, VoiceServerUpdate},
};
use conn::{Connection, Session};

/// A music player for a specific guild.
pub struct Player {
    guild_id: Id<GuildMarker>,
    user_id: Id<UserMarker>,
    gateway_tx: UnboundedSender<GatewayEvent>,
}

impl Player {
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
            user_id,
        }
    }

    pub fn voice_state_update(&self, ev: Box<VoiceStateUpdate>) {
        self.gateway_tx.send(GatewayEvent::VoiceStateUpdate(ev)).unwrap();
    }

    pub fn voice_server_update(&self, ev: VoiceServerUpdate) {
        self.gateway_tx.send(GatewayEvent::VoiceServerUpdate(ev)).unwrap();
    }
}

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
            guild_id, user_id,
            endpoint: vseu.endpoint.unwrap(),
            token: vseu.token,
            session_id: vstu.0.session_id,
        }
    } else {
        error!("got not response for voice state update");
        return;
    };

    debug!("got new session; establishing ws connection...");

    let timeout = Instant::now() + Duration::from_millis(5000);
    let mut conn = match timeout_at(timeout, Connection::connect(session)).await {
        Ok(Ok(conn)) => conn,
        Ok(Err(err)) => {
            error!("failed to connect to endpoint: {}", err);
            return;
        }
        Err(_) => {
            error!("player initialization timed out after 5000ms!");
            return;
        }
    };

    loop {
        tokio::select! {
            ev = conn.next() => {
                debug!("ev: {:?}", ev);
            }
        }
    }
}

