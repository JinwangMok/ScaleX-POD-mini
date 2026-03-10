use thiserror::Error;

#[derive(Error, Debug)]
pub enum ScalexError {
    #[error("Configuration error: {0}")]
    Config(String),

    #[error("SSH error on host '{host}': {detail}")]
    Ssh { host: String, detail: String },

    #[error("Failed to parse facts from host '{host}': {detail}")]
    FactsParse { host: String, detail: String },

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("YAML error: {0}")]
    Yaml(#[from] serde_yaml::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Environment variable '{name}' not found")]
    EnvVar { name: String },

    #[error("File not found: {0}")]
    FileNotFound(String),
}
