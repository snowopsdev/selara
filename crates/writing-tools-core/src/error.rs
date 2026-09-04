use thiserror::Error;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("config error: {0}")]
    Config(String),
    #[error("unknown command: {0}")]
    UnknownCommand(String),
    #[error("provider error: {0}")]
    Provider(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Http(#[from] reqwest::Error),
    #[error(transparent)]
    Toml(#[from] toml::de::Error),
}
