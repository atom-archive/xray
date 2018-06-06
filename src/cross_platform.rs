use std::borrow::Cow;
use std::ffi::{OsStr, OsString};

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum PathComponent {
    Unix(Vec<u8>),
}

impl PathComponent {
    pub fn to_string_lossy(&self) -> Cow<str> {
        match self {
            &PathComponent::Unix(ref chars) => String::from_utf8_lossy(&chars),
        }
    }
}

#[cfg(unix)]
impl From<OsString> for PathComponent {
    fn from(string: OsString) -> Self {
        use std::os::unix::ffi::OsStringExt;
        PathComponent::Unix(string.into_vec())
    }
}

#[cfg(unix)]
impl<'a> From<&'a OsStr> for PathComponent {
    fn from(string: &'a OsStr) -> Self {
        use std::os::unix::ffi::OsStrExt;
        PathComponent::Unix(string.as_bytes().to_owned())
    }
}

#[cfg(test)]
impl<'a> From<&'a str> for PathComponent {
    #[cfg(unix)]
    fn from(string: &'a str) -> Self {
        PathComponent::Unix(string.as_bytes().to_owned())
    }
}
