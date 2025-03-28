use std::{fmt::Display, ops::Deref};

pub type CloneableResult<T> = Result<T, CloneableError>;

pub struct CloneableError(anyhow::Error);

impl CloneableError {
    pub fn new<E: std::error::Error + Send + Sync + 'static>(error: E) -> Self {
        Self(anyhow::Error::new(error))
    }
}

impl<T: Into<anyhow::Error> + Display> From<T> for CloneableError {
    fn from(error: T) -> Self {
        Self(error.into())
    }
}

impl From<CloneableError> for anyhow::Error {
    fn from(error: CloneableError) -> Self {
        error.0
    }
}

impl Clone for CloneableError {
    fn clone(&self) -> Self {
        Self(anyhow::anyhow!(self.0.to_string()))
    }
}

impl Deref for CloneableError {
    type Target = anyhow::Error;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
