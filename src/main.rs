use log::LevelFilter;
use std::{env, sync::Arc};
use twilight_gateway::{Cluster, Intents};
use futures_util::StreamExt;

use swc::player::Player;
use twilight_model::gateway::{payload::outgoing::update_voice_state::UpdateVoiceState, event::Event};
use twilight_model::id::{Id, marker::{ChannelMarker, GuildMarker, UserMarker}};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenv::dotenv().ok();
    env_logger::Builder::new()
        .filter_level(LevelFilter::Debug)
        .parse_default_env()
        .init();

    // initialize discord cluster
    let (cluster, mut events) = Cluster::new(env::var("DISCORD_TOKEN")?, Intents::GUILD_VOICE_STATES).await?;
    let cluster = Arc::new(cluster);

    // spawn cluster thread
    let cluster_spawn = Arc::clone(&cluster);
    tokio::spawn(async move {
        cluster_spawn.up().await;
    });

    let mut player: Option<Player> = None;

    while let Some((_, ev)) = events.next().await {
        match ev {
            Event::Ready(_) => {
                let guild_id = Id::<GuildMarker>::new(952331087714070548);
                let channel_id = Id::<ChannelMarker>::new(972610486229168244);
                let user_id = Id::<UserMarker>::new(895420881696849920);

                // create player
                player = Some(Player::new(user_id, guild_id).await);

                // establish connection
                cluster.command(0, &UpdateVoiceState::new(guild_id, channel_id, false, false)).await.unwrap();
            }
            Event::VoiceStateUpdate(ev) => {
                if let Some(player) = &player {
                    player.voice_state_update(ev);
                }
            }
            Event::VoiceServerUpdate(ev) => {
                if let Some(player) = &player {
                    player.voice_server_update(ev);
                }
            }
            _ => ()
        }
    }

    Ok(())
}

