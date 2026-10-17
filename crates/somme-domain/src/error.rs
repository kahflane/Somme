#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum DomainError {
    #[error("{0} is required")]
    Required(&'static str),
    #[error("{0}")]
    Invalid(String),
}

#[derive(Debug, thiserror::Error)]
pub enum RepositoryError {
    #[error("storage error: {0}")]
    Storage(String),
    #[error("corrupt record in {table}: {reason}")]
    Corrupt { table: &'static str, reason: String },
}
