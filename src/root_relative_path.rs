use std::{path::{PathBuf, Path}, fmt::{Display, self}};

use console::Style;
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

pub enum Side {
    Source,
    Dest
}

/// For user-friendly display of a RootRelativePath on the source or dest.
/// Formats a path which is relative to the root, so that it is easier to understand for the user.
/// Especially if path is empty (i.e. referring to the root itself)
pub struct PrettyPath<'a> {
    pub side: Side,
    pub dir_separator: char,
    pub root: &'a str,
    pub path: &'a RootRelativePath,
    pub kind: &'static str, // e.g. 'folder', 'file'
}
impl<'a> Display for PrettyPath<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let side = match self.side {
            Side::Source { .. } => "source",
            Side::Dest { .. } => "dest",
        };
        let root = self.root;
        let path = self.path;
        let kind = self.kind;

        // Use styling to highlight which part of the path is the root, and which is the root-relative path.
        // We don't play with any characters in the path (e.g. adding brackets) so that the user can copy-paste the 
        // full paths if they want
        // The styling plays nicely with piping output to a file, as they are simply ignored (part of the `console` crate)
        let root_style = Style::new().italic();
        if self.path.is_root() {
            write!(f, "{side} root {kind} '{}'", root_style.apply_to(root))
        } else {
            let root_with_trailing_slash = if root.ends_with(self.dir_separator) {
                root.to_string()
            } else {
                root.to_string() + &self.dir_separator.to_string()
            };
            // Convert the path from normalized (forward slashes) to the native representation for that platform
            let path_with_appropriate_slashes = path.to_platform_path(self.dir_separator);
            write!(f, "{side} {kind} '{}{path_with_appropriate_slashes}'", root_style.apply_to(root_with_trailing_slash))
        }
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