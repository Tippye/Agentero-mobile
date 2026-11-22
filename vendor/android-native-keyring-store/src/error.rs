#[derive(thiserror::Error, Debug)]
pub enum AndroidKeyringError {
    #[error(transparent)]
    JniError(#[from] jni::errors::Error),
    #[error("Java exception was thrown")]
    JavaExceptionThrow,
    #[error("Android credential changes were not persisted")]
    PersistenceFailure,
    #[error("{1}")]
    CorruptedData(Vec<u8>, CorruptedData),
    #[error(transparent)]
    KeyringError(#[from] keyring_core::Error),
}

impl From<AndroidKeyringError> for keyring_core::Error {
    fn from(value: AndroidKeyringError) -> Self {
        match value {
            AndroidKeyringError::JniError(error) => {
                keyring_core::Error::PlatformFailure(Box::new(error))
            }
            e @ AndroidKeyringError::PersistenceFailure => {
                keyring_core::Error::PlatformFailure(Box::new(e))
            }
            e @ AndroidKeyringError::JavaExceptionThrow => {
                keyring_core::Error::PlatformFailure(Box::new(e))
            }
            AndroidKeyringError::CorruptedData(data, error) => {
                keyring_core::Error::BadDataFormat(data, Box::new(error))
            }
            AndroidKeyringError::KeyringError(error) => error,
        }
    }
}

pub type AndroidKeyringResult<T> = Result<T, AndroidKeyringError>;

#[derive(thiserror::Error, Debug)]
pub enum CorruptedData {
    #[error("Stored credential encoding is invalid")]
    InvalidBase64,
    #[error("IV length not specified on entry")]
    MissingIvLen,
    #[error("IV length in data is {actual}, but should be {expected}")]
    InvalidIvLen { actual: usize, expected: usize },
    #[error("Data is too small to contain IV and ciphertext, length = {0}")]
    DataTooSmall(usize),
    #[error("Verification of data signature/MAC failed")]
    DecryptionFailure,
}
