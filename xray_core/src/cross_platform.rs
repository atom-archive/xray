use std::borrow::Cow;
#[cfg(unix)]
use std::ffi::{OsStr, OsString};
#[cfg(unix)]
use std::path::PathBuf;

pub const UNIX_MAIN_SEPARATOR: u8 = b'/';

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
pub enum PathComponent {
    Unix(Vec<u8>),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Path(Option<PathState>);

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
enum PathState {
    Unix(Vec<u8>),
}

impl PathComponent {
    pub fn to_string_lossy(&self) -> Cow<str> {
        match self {
            &PathComponent::Unix(ref chars) => String::from_utf8_lossy(&chars),
        }
    }
}

impl Path {
    pub fn new() -> Self {
        Path(None)
    }

    pub fn push(&mut self, component: &PathComponent) {
        if let Some(ref mut path) = self.0 {
            match path {
                &mut PathState::Unix(ref mut path_chars) => {
                    let &PathComponent::Unix(ref component_chars) = component;
                    if path_chars.len() != 0 {
                        path_chars.push(UNIX_MAIN_SEPARATOR);
                    }
                    path_chars.extend(component_chars);
                }
            }
        } else {
            match component {
                &PathComponent::Unix(ref chars) => self.0 = Some(PathState::Unix(chars.clone())),
            }
        }
    }

    pub fn push_path(&mut self, other: &Self) {
        if let Some(ref mut path) = self.0 {
            match path {
                &mut PathState::Unix(ref mut path_chars) => {
                    if let Some(ref other) = other.0 {
                        let &PathState::Unix(ref component_chars) = other;
                        if path_chars.len() != 0 {
                            path_chars.push(UNIX_MAIN_SEPARATOR);
                        }
                        path_chars.extend(component_chars);
                    }
                }
            }
        } else {
            *self = other.clone();
        }
    }

    #[cfg(unix)]
    pub fn to_path_buf(&self) -> PathBuf {
        use std::os::unix::ffi::OsStrExt;

        if let Some(ref path) = self.0 {
            match path {
                &PathState::Unix(ref chars) => OsStr::from_bytes(chars).into(),
            }
        } else {
            PathBuf::new()
        }
    }

    #[cfg(test)]
    pub fn to_string_lossy(&self) -> String {
        if let Some(ref path) = self.0 {
            match path {
                &PathState::Unix(ref chars) => String::from_utf8_lossy(chars).into_owned(),
            }
        } else {
            String::new()
        }
    }
}

#[cfg(unix)]
impl From<OsString> for Path {
    fn from(path: OsString) -> Self {
        use std::os::unix::ffi::OsStringExt;
        Path(Some(PathState::Unix(path.into_vec())))
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

#[cfg(test)]
impl<'a> From<&'a str> for Path {
    #[cfg(unix)]
    fn from(string: &'a str) -> Self {
        Path(Some(PathState::Unix(string.as_bytes().to_owned())))
    }
}
