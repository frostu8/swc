use futures_util::StreamExt;
use log::LevelFilter;
use std::{env, sync::Arc};
use twilight_gateway::{Cluster, Intents};
use twilight_http::Client;

use swc::player::{Manager, audio::Track, commands::{Command, CommandType}};

use twilight_model::{
    application::interaction::{
        application_command::{CommandData, CommandOptionValue},
        Interaction, InteractionData,
    },
    gateway::event::Event, 
    id::Id,
};

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

    // create http client
    let http_client = Arc::new(Client::new(env::var("DISCORD_TOKEN")?));

    let mut manager: Option<Manager> = None;

    while let Some((_, ev)) = events.next().await {
        match ev {
            Event::Ready(ready) => {
                let user_id = ready.user.id;
                let application_id = ready.application.id;

                // initialize manager
                manager = Some(Manager::new(user_id, application_id, http_client.clone(), Arc::clone(&cluster)));

                let player = manager
                    .as_ref()
                    .unwrap()
                    .join(Id::new(683483117473759249), Id::new(683483410962055270))
                    .await;

                //player.push(Source::ytdl("https://youtu.be/vqzMdWcwSQs").await.unwrap()).unwrap();
            }
            Event::InteractionCreate(mut interaction) => {
                match interaction.data.take() {
                    Some(InteractionData::ApplicationCommand(data)) => {
                        tokio::spawn(handle(manager.clone().unwrap(), interaction.0, data));
                    }
                    _ => ()
                }
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

/// Handles execution of a command.
pub async fn handle(
    manager: Manager,
    interaction: Interaction,
    data: Box<CommandData>,
) {
    match data.name.as_str() {
        "play" => {
            // first argument is always url
            let url = &data
                .options
                .get(0)
                .expect("invalid command schema")
                .value;

            let url = match url {
                CommandOptionValue::String(url) => url,
                _ => panic!("invalid command schema"),
            };

            // TODO: gracefully handle error
            let track = Track::ytdl(url).await.unwrap();

            // TODO: handle missing player
            let player = manager
                .get(interaction.guild_id.expect("guild_id in command"))
                .await
                .unwrap();

            player.command(Command {
                interaction: interaction.into(),
                kind: CommandType::Play(track),
            })
                .unwrap();
        }
        _ => log::warn!("unknown command /{}", data.name)
    }
}
