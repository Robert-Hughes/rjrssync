use std::{path::{PathBuf, Path}, fmt::{Display, self}};

use regex::{RegexSet, SetMatches};
use serde::{Serialize, Deserialize};


/// Converts a platform-specific relative path (inside the source or dest root)
/// to something that can be sent over our comms. We can't simply use PathBuf
/// because the syntax of this path might differ between the boss and doer
/// platforms (e.g. Windows vs Linux), and so the type might have different
/// meaning/behaviour on each side.
/// We instead convert to a normalized representation using forward slashes (i.e. Unix-style).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Hash)]
pub struct RootRelativePath {
    inner: String,
}
impl RootRelativePath {  
    pub fn root() -> RootRelativePath {
        RootRelativePath { inner: "".to_string() }
    }

    /// Does this path refer to the root itself?
    pub fn is_root(&self) -> bool {
        self.inner.is_empty()
    }

    /// Gets the full path consisting of the root and this root-relative path.
    pub fn get_full_path(&self, root: &Path) -> PathBuf {
        if self.is_root() { root.to_path_buf() } else { root.join(&self.inner) }
    }

    /// Rather than exposing the inner string, expose just regex matching.
    /// This reduces the risk of incorrect usage of the raw string value (e.g. by using
    /// local-platform Path functions).
    pub fn regex_set_matches(&self, r: &RegexSet) -> SetMatches {
        r.matches(&self.inner)
    }

    /// Puts the slashes back to what is requested, so that the path is appropriate for
    /// another platform.
    pub fn to_platform_path(&self, dir_separator: char) -> String {
        self.inner.replace('/', &dir_separator.to_string())
    }
}
impl Display for RootRelativePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_root() {
            write!(f, "<ROOT>")
        } else {
            write!(f, "{}", self.inner)
        }
    }
}
impl TryFrom<&Path> for RootRelativePath {
    type Error = String;

    fn try_from(p: &Path) -> Result<RootRelativePath, String> {
        if p.is_absolute() {
            return Err("Must be relative".to_string());
        }
    
        let mut result = String::new();
        for c in p.iter() {
            let cs = match c.to_str() {
                Some(x) => x,
                None => return Err("Can't convert path component".to_string()),
            };
            if cs.contains('/') || cs.contains('\\') {
                // Slashes in any component would mess things up, once we change which slash is significant
                return Err("Illegal characters in path".to_string());
            }
            if !result.is_empty() {
                result += "/";
            }
            result += cs;
        }
    
        Ok(RootRelativePath { inner: result })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_path_is_root() {
        let x = RootRelativePath::try_from(Path::new(""));
        assert_eq!(x, Ok(RootRelativePath::root()));
        assert_eq!(x.unwrap().is_root(), true);
    }

    #[test]
    fn test_normalize_path_absolute() {
        let x = if cfg!(windows) {
            "C:\\Windows"
        } else {
            "/etc/hello"
        };
        assert_eq!(RootRelativePath::try_from(Path::new(x)), Err("Must be relative".to_string()));
    }

    #[cfg(unix)] // This test isn't possible on Windows, because both kinds of slashes are valid separators
    #[test]
    fn test_normalize_path_slashes_in_component() {
        assert_eq!(RootRelativePath::try_from(Path::new("a path with\\backslashes/adsa")), Err("Illegal characters in path".to_string()));
    }

    #[test]
    fn test_normalize_path_multiple_components() {
        assert_eq!(RootRelativePath::try_from(Path::new("one/two/three")), Ok(RootRelativePath { inner: "one/two/three".to_string() }));
    }
}