use crate::fs::models::{FsError, FsResult};

#[cfg(any(
    target_os = "freebsd",
    target_os = "ios",
    target_os = "linux",
    target_os = "macos",
    target_os = "openbsd",
    target_os = "windows"
))]
const REMOTE_CREDENTIAL_SERVICE: &str = "dev.carelo.remote-volumes.v1";

pub(super) trait RemoteCredentialStore: Send + Sync {
    fn store(&self, reference: &str, secret: &str) -> FsResult<()>;
    fn load(&self, reference: &str) -> FsResult<Option<String>>;
    fn delete(&self, reference: &str) -> FsResult<()>;
}

#[derive(Debug, Default)]
pub(super) struct OsRemoteCredentialStore;

#[cfg(any(
    target_os = "freebsd",
    target_os = "ios",
    target_os = "linux",
    target_os = "macos",
    target_os = "openbsd",
    target_os = "windows"
))]
impl OsRemoteCredentialStore {
    fn entry(reference: &str) -> FsResult<keyring::Entry> {
        keyring::Entry::new(REMOTE_CREDENTIAL_SERVICE, reference).map_err(|error| {
            credential_error(
                "Unable to access the operating system credential store",
                error,
            )
        })
    }
}

#[cfg(any(
    target_os = "freebsd",
    target_os = "ios",
    target_os = "linux",
    target_os = "macos",
    target_os = "openbsd",
    target_os = "windows"
))]
impl RemoteCredentialStore for OsRemoteCredentialStore {
    fn store(&self, reference: &str, secret: &str) -> FsResult<()> {
        Self::entry(reference)?
            .set_password(secret)
            .map_err(|error| {
                credential_error(
                    "Unable to save remote credentials in the operating system credential store",
                    error,
                )
            })
    }

    fn load(&self, reference: &str) -> FsResult<Option<String>> {
        match Self::entry(reference)?.get_password() {
            Ok(secret) => Ok(Some(secret)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(credential_error(
                "Unable to read remote credentials from the operating system credential store",
                error,
            )),
        }
    }

    fn delete(&self, reference: &str) -> FsResult<()> {
        match Self::entry(reference)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(credential_error(
                "Unable to remove remote credentials from the operating system credential store",
                error,
            )),
        }
    }
}

#[cfg(not(any(
    target_os = "freebsd",
    target_os = "ios",
    target_os = "linux",
    target_os = "macos",
    target_os = "openbsd",
    target_os = "windows"
)))]
impl RemoteCredentialStore for OsRemoteCredentialStore {
    fn store(&self, _reference: &str, _secret: &str) -> FsResult<()> {
        Err(unsupported_credential_store())
    }

    fn load(&self, _reference: &str) -> FsResult<Option<String>> {
        Err(unsupported_credential_store())
    }

    fn delete(&self, _reference: &str) -> FsResult<()> {
        Err(unsupported_credential_store())
    }
}

#[cfg(any(
    target_os = "freebsd",
    target_os = "ios",
    target_os = "linux",
    target_os = "macos",
    target_os = "openbsd",
    target_os = "windows"
))]
fn credential_error(action: &str, error: keyring::Error) -> FsError {
    FsError::new(
        "credential_store_unavailable",
        format!("{action}: {error}"),
        None,
    )
}

#[cfg(not(any(
    target_os = "freebsd",
    target_os = "ios",
    target_os = "linux",
    target_os = "macos",
    target_os = "openbsd",
    target_os = "windows"
)))]
fn unsupported_credential_store() -> FsError {
    FsError::new(
        "credential_store_unavailable",
        "This platform does not provide a supported operating system credential store.",
        None,
    )
}
