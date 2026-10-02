//! One plain entry name: the only name a held directory handle opens.

use std::ffi::{OsStr, OsString};

use crate::OpenRefusal;

/// One plain path component a held directory handle opens relative to itself.
///
/// Never empty, `.`, `..`, or holding a separator or NUL; on Windows also
/// never a name Win32 reads as another entry, a stream, or a device
/// ([`crate::win32_name::is_verbatim_entry_name`]), and always valid Unicode.
/// Every entry act takes one, so an act can name only an entry directly
/// inside the held directory.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct EntryName(OsString);

impl EntryName {
    /// The single component `name`, or `None` when it is not one.
    #[must_use]
    pub fn new(name: &OsStr) -> Option<Self> {
        (is_component(name) && platform_admits(name)).then(|| Self(name.to_os_string()))
    }

    /// The single component `name`.
    ///
    /// # Errors
    /// [`OpenRefusal::BadName`] when it is not one.
    pub fn parse(name: &OsStr) -> Result<Self, OpenRefusal> {
        Self::new(name).ok_or(OpenRefusal::BadName)
    }

    /// The name as an OS string.
    #[must_use]
    pub fn as_os_str(&self) -> &OsStr {
        &self.0
    }
}

impl AsRef<OsStr> for EntryName {
    fn as_ref(&self) -> &OsStr {
        &self.0
    }
}

/// Whether `name` is one component: not empty, `.`, or `..`, and free of separators and NUL.
fn is_component(name: &OsStr) -> bool {
    let bytes = name.as_encoded_bytes();
    !bytes.is_empty()
        && bytes != b"."
        && bytes != b".."
        && !bytes
            .iter()
            .any(|&b| b == 0 || std::path::is_separator(char::from(b)))
}

/// Whether Win32 opens `name` relative to a handle as exactly the entry it spells.
///
/// A name that is not valid Unicode cannot be checked, so it is refused.
#[cfg(windows)]
fn platform_admits(name: &OsStr) -> bool {
    name.to_str()
        .is_some_and(crate::win32_name::is_verbatim_entry_name)
}

/// Every component is a plain name outside Windows.
#[cfg(not(windows))]
const fn platform_admits(_name: &OsStr) -> bool {
    true
}
