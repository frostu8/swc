//! Bot commands.

use crate::player::{Manager, queue::Source};

use anyhow::Context as _;

use twilight_model::application::interaction::{
    application_command::{CommandData, CommandOptionValue}, Interaction,
};

/// Command execution context.
pub struct Context {
    pub audio_manager: Manager,
}

impl Context {
    pub fn new(
        audio_manager: Manager,
    ) -> Context {
        Context {
            audio_manager,
        }
    }
}

/// Handles execution of a command.
pub async fn handle(
    cx: Context,
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

            play(cx, interaction, url).await
                .context("failed to execute /play")
                .unwrap()
        }
        _ => warn!("unknown command /{}", data.name)
    }
}

/// The `/play` command.
pub async fn play(
    cx: Context,
    interaction: Interaction,
    url: &str,
) -> Result<(), anyhow::Error> {
    // TODO: handle missing player
    let player = cx.audio_manager
        .get(interaction.guild_id.expect("guild_id in command"))
        .await
        .unwrap();

    let source = Source::ytdl(url).await?;

    player.push(source).unwrap();

    Ok(())
}
