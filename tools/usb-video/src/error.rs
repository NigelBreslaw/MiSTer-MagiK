pub type AgentResult<T> = Result<T, AgentError>;
#[derive(Clone, Debug, thiserror::Error)]
pub enum AgentError {
    #[error("{code}: {detail}")]
    Classified { code: &'static str, detail: String },
    #[error("{0}")]
    Message(String),
}
impl From<std::io::Error> for AgentError {
    fn from(error: std::io::Error) -> Self {
        Self::Message(error.to_string())
    }
}
impl From<String> for AgentError {
    fn from(error: String) -> Self {
        Self::Message(error)
    }
}
impl From<&str> for AgentError {
    fn from(error: &str) -> Self {
        Self::Message(error.to_owned())
    }
}
