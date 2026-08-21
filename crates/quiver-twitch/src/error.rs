#[derive(Debug, thiserror::Error)]
pub enum TwitchError {
    #[error("invalid channel login: {0}")]
    Validate(#[from] twitch_irc::validate::Error),
}
