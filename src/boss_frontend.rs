#[derive(clap::Parser)]
pub struct BossCliArgs {
    /// The source path, which will be synced to the destination path.
    /// Optionally contains a username and hostname for specifying remote paths.
    /// Format: [[username@]hostname:]path
    pub src: RemotePathDesc,
    /// The destination path, which will be synced from the source path.
    /// Optionally contains a username and hostname for specifying remote paths.
    /// Format: [[username@]hostname:]path
    pub dest: RemotePathDesc,
    /// If set, forces redeployment of rjrssync to any remote targets, even if they already have an
    /// up-to-date copy.
    #[arg(long)]
    pub force_redeploy: bool,
    #[arg(name="exclude", long)]
    pub exclude_filters: Vec<String>,
    /// Override the port used to connect to hostnames specified in src or dest.
    #[arg(long, default_value_t = 40129)]
    pub remote_port: u16,
    
    #[arg(long)]
    pub dry_run: bool,

    /// Outputs some additional statistics about the data copied.
    #[arg(long)]
    pub stats: bool, // This is a separate flag to --verbose, because that is more for debugging, but this is useful for normal users
    /// Hides all output except warnings and errors. 
    #[arg(short, long, group="verbosity")]
    pub quiet: bool,
    /// Shows additional output.
    #[arg(short, long, group="verbosity")]
    pub verbose: bool,

    /// [Internal] Launches as a doer process, rather than a boss process.
    /// This shouldn't be needed for regular operation.
    #[arg(long)]
    pub doer: bool,
}

/// Describes a local or remote path, parsed from the `src` or `dest` command-line arguments.
#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub struct RemotePathDesc {
    pub username: String,
    pub hostname: String,
    // Note this shouldn't be a PathBuf, because the syntax of this path will be for the remote system,
    // which might be different to the local system.
    pub path: String,
}
impl std::str::FromStr for RemotePathDesc {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // There's some quirks here with windows paths containing colons for drive letters

        let mut r = RemotePathDesc::default();

        // The first colon splits path from the rest, apart from special case for drive letters
        match s.split_once(':') {
            None => {
                r.path = s.to_string();
            }
            Some((a, b)) if a.len() == 1 && (b.is_empty() || b.starts_with('\\')) => {
                r.path = s.to_string();
            }
            Some((user_and_host, path)) => {
                r.path = path.to_string();

                // The first @ splits the user and hostname
                match user_and_host.split_once('@') {
                    None => {
                        r.hostname = user_and_host.to_string();
                    }
                    Some((user, host)) => {
                        r.username = user.to_string();
                        if r.username.is_empty() {
                            return Err("Missing username".to_string());
                        }
                        r.hostname = host.to_string();
                    }
                };
                if r.hostname.is_empty() {
                    return Err("Missing hostname".to_string());
                }
            }
        };

        if r.path.is_empty() {
            return Err("Path must be specified".to_string());
        }

        Ok(r)
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;

    #[test]
    fn parse_remote_path_desc() {
        // There's some quirks here with windows paths containing colons for drive letters

        assert_eq!(
            RemotePathDesc::from_str(""),
            Err("Path must be specified".to_string())
        );
        assert_eq!(
            RemotePathDesc::from_str("f"),
            Ok(RemotePathDesc {
                path: "f".to_string(),
                ..Default::default()
            })
        );
        assert_eq!(
            RemotePathDesc::from_str("h:f"),
            Ok(RemotePathDesc {
                path: "f".to_string(),
                hostname: "h".to_string(),
                username: "".to_string()
            })
        );
        assert_eq!(
            RemotePathDesc::from_str("hh:"),
            Err("Path must be specified".to_string())
        );
        assert_eq!(
            RemotePathDesc::from_str(":f"),
            Err("Missing hostname".to_string())
        );
        assert_eq!(
            RemotePathDesc::from_str(":"),
            Err("Missing hostname".to_string())
        );
        assert_eq!(
            RemotePathDesc::from_str("@"),
            Ok(RemotePathDesc {
                path: "@".to_string(),
                ..Default::default()
            })
        );

        assert_eq!(
            RemotePathDesc::from_str("u@h:f"),
            Ok(RemotePathDesc {
                path: "f".to_string(),
                hostname: "h".to_string(),
                username: "u".to_string()
            })
        );
        assert_eq!(
            RemotePathDesc::from_str("@h:f"),
            Err("Missing username".to_string())
        );
        assert_eq!(
            RemotePathDesc::from_str("u@h:"),
            Err("Path must be specified".to_string())
        );
        assert_eq!(
            RemotePathDesc::from_str("u@:f"),
            Err("Missing hostname".to_string())
        );
        assert_eq!(
            RemotePathDesc::from_str("@:f"),
            Err("Missing username".to_string())
        );
        assert_eq!(
            RemotePathDesc::from_str("u@:"),
            Err("Missing hostname".to_string())
        );
        assert_eq!(
            RemotePathDesc::from_str("@h:"),
            Err("Missing username".to_string())
        );

        assert_eq!(
            RemotePathDesc::from_str("u@f"),
            Ok(RemotePathDesc {
                path: "u@f".to_string(),
                ..Default::default()
            })
        );
        assert_eq!(
            RemotePathDesc::from_str("@f"),
            Ok(RemotePathDesc {
                path: "@f".to_string(),
                ..Default::default()
            })
        );
        assert_eq!(
            RemotePathDesc::from_str("u@"),
            Ok(RemotePathDesc {
                path: "u@".to_string(),
                ..Default::default()
            })
        );

        assert_eq!(
            RemotePathDesc::from_str("u:u@u:u@h:f:f:f@f"),
            Ok(RemotePathDesc {
                path: "u@u:u@h:f:f:f@f".to_string(),
                hostname: "u".to_string(),
                username: "".to_string()
            })
        );

        assert_eq!(
            RemotePathDesc::from_str(r"C:\Path\On\Windows"),
            Ok(RemotePathDesc {
                path: r"C:\Path\On\Windows".to_string(),
                ..Default::default()
            })
        );
        assert_eq!(
            RemotePathDesc::from_str(r"C:"),
            Ok(RemotePathDesc {
                path: r"C:".to_string(),
                ..Default::default()
            })
        );
        assert_eq!(
            RemotePathDesc::from_str(r"C:\"),
            Ok(RemotePathDesc {
                path: r"C:\".to_string(),
                ..Default::default()
            })
        );
        assert_eq!(
            RemotePathDesc::from_str(r"C:folder"),
            Ok(RemotePathDesc {
                path: r"folder".to_string(),
                hostname: "C".to_string(),
                ..Default::default()
            })
        );
        assert_eq!(
            RemotePathDesc::from_str(r"C:\folder"),
            Ok(RemotePathDesc {
                path: r"C:\folder".to_string(),
                ..Default::default()
            })
        );
        assert_eq!(
            RemotePathDesc::from_str(r"CC:folder"),
            Ok(RemotePathDesc {
                path: r"folder".to_string(),
                hostname: "CC".to_string(),
                ..Default::default()
            })
        );
        assert_eq!(
            RemotePathDesc::from_str(r"CC:\folder"),
            Ok(RemotePathDesc {
                path: r"\folder".to_string(),
                hostname: "CC".to_string(),
                ..Default::default()
            })
        );
        assert_eq!(
            RemotePathDesc::from_str(r"s:C:\folder"),
            Ok(RemotePathDesc {
                path: r"C:\folder".to_string(),
                hostname: "s".to_string(),
                ..Default::default()
            })
        );
        assert_eq!(
            RemotePathDesc::from_str(r"u@s:C:\folder"),
            Ok(RemotePathDesc {
                path: r"C:\folder".to_string(),
                hostname: "s".to_string(),
                username: "u".to_string()
            })
        );

        assert_eq!(
            RemotePathDesc::from_str(r"\\network\share\windows"),
            Ok(RemotePathDesc {
                path: r"\\network\share\windows".to_string(),
                ..Default::default()
            })
        );

        assert_eq!(
            RemotePathDesc::from_str("/unix/absolute"),
            Ok(RemotePathDesc {
                path: "/unix/absolute".to_string(),
                ..Default::default()
            })
        );
        assert_eq!(
            RemotePathDesc::from_str("username@server:/unix/absolute"),
            Ok(RemotePathDesc {
                path: "/unix/absolute".to_string(),
                hostname: "server".to_string(),
                username: "username".to_string()
            })
        );
    }
}
