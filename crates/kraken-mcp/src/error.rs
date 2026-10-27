use thiserror::Error;

pub type Result<T> = std::result::Result<T, McpError>;

/// Failure modes of server assembly and serving. Tool-call failures never
/// surface here: they return to the MCP client as tool results.
#[derive(Debug, Error)]
pub enum McpError {
    /// Rejected input: an unknown service filter or an empty registry.
    #[error("{0}")]
    Validation(String),

    /// The stdio transport failed to start or serve.
    #[error("{0}")]
    Transport(String),
}
