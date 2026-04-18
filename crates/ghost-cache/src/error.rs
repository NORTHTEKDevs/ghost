#[derive(thiserror::Error, Debug)]
pub enum CacheError {
    #[error("sqlite: {0}")]
    Sqlite(String),

    #[error("io: {0}")]
    Io(String),

    #[error("stub")]
    Stub,
}
