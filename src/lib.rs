//! Soundwave command library.

//pub mod player;
pub mod interaction;
pub mod music;
pub mod voice;
pub mod ytdl;

use twilight_model::application::command::{
    Command, CommandOption, CommandOptionType, CommandType,
};
use twilight_model::id::Id;

/// Returns a chat command with a name and description.
///
/// This makes for easy autocompletion with struct flattening:
/// ```
/// # use twilight_model::application::command::{Command, CommandOptionType};
/// # use swc::{command, command_option};
/// let command = Command {
///     options: vec![
///         command_option(
///             CommandOptionType::String,
///             "track",
///             "the track to play",
///         ),
///     ],
///     ..command("play", "plays a track")
/// };
/// ```
pub fn command(name: impl Into<String>, description: impl Into<String>) -> Command {
    Command {
        application_id: None,
        default_member_permissions: None,
        dm_permission: None,
        description: description.into(),
        description_localizations: None,
        guild_id: None,
        id: None,
        kind: CommandType::ChatInput,
        name: name.into(),
        name_localizations: None,
        options: Vec::new(),
        version: Id::new(1),
        nsfw: Some(false),
    }
}

/// Creates a new, **required** command option.
pub fn command_option(
    kind: CommandOptionType,
    name: impl Into<String>,
    description: impl Into<String>,
) -> CommandOption {
    CommandOption {
        autocomplete: None,
        channel_types: None,
        choices: None,
        description: description.into(),
        description_localizations: None,
        kind,
        max_length: None,
        max_value: None,
        min_length: None,
        min_value: None,
        name: name.into(),
        name_localizations: None,
        options: None,
        required: Some(true),
    }
}

/// Creates a list of commands the bot supports.
pub fn commands() -> Vec<Command> {
    vec![
        Command {
            options: vec![command_option(
                CommandOptionType::String,
                "query",
                "the url or query of the track",
            )],
            ..command("play", "play a music track")
        },
        Command {
            options: vec![command_option(
                CommandOptionType::String,
                "query",
                "the url or query of the track",
            )],
            ..command(
                "playnow",
                "play a music track and moves it to the top of the queue",
            )
        },
        command("skip", "skips the currently playing song"),
        command("queue", "lists the current music queue"),
        command("shuffle", "shuffles the music queue"),
        command("disconnect", "disconnects the music bot"),
        Command {
            options: vec![command_option(
                CommandOptionType::Boolean,
                "setting",
                "whether to autodisconnect or not",
            )],
            ..command(
                "autodisconnect",
                "sets the autodisconnect setting; omit setting to toggle",
            )
        },
    ]
}
