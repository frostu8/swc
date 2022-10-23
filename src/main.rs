use futures_util::StreamExt;
use log::LevelFilter;
use std::{env, sync::Arc};
use twilight_gateway::{Cluster, Intents};

use swc::player::Manager;
use twilight_model::gateway::event::Event;
use twilight_model::id::Id;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenv::dotenv().ok();
    env_logger::Builder::new()
        .filter_level(LevelFilter::Debug)
        .parse_default_env()
        .init();

    // initialize discord cluster
    let (cluster, mut events) =
        Cluster::new(env::var("DISCORD_TOKEN")?, Intents::GUILD_VOICE_STATES).await?;
    let cluster = Arc::new(cluster);

    // spawn cluster thread
    let cluster_spawn = Arc::clone(&cluster);
    tokio::spawn(async move {
        cluster_spawn.up().await;
    });

    let mut manager: Option<Manager> = None;

    while let Some((_, ev)) = events.next().await {
        match ev {
            Event::Ready(ready) => {
                let user_id = ready.user.id;

                // initialize manager
                manager = Some(Manager::new(Arc::clone(&cluster), user_id));
                manager
                    .as_ref()
                    .unwrap()
                    .join(Id::new(683483117473759249), Id::new(683483410962055270))
                    .await;
            }
            Event::VoiceStateUpdate(ev) => {
                if let Some(manager) = &manager {
                    manager.voice_state_update(ev).await;
                }
            }
            Event::VoiceServerUpdate(ev) => {
                if let Some(manager) = &manager {
                    manager.voice_server_update(ev).await;
                }
            }
            _ => (),
        }
    }

    Ok(())
}
