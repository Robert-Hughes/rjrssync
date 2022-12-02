use std::path::Path;
use std::process::ExitCode;
use std::io::Write;

use clap::{Parser, ValueEnum};
use env_logger::{Env, fmt::Color};
use log::info;
use log::{debug, error};
use yaml_rust::{YamlLoader, Yaml};

use crate::boss_launch::*;
use crate::boss_sync::*;
use crate::doer::Filter;

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, ValueEnum)]
pub enum SymlinkMode {
    /// Symlinks are treated as if they are the target that they point to. No special treatment is given.
    Unaware,
    /// Symlinks are treated as if they were simple text files containing their target address.
    /// They are not followed or validated. They will be reproduced as accurately as possible on
    /// the destination.
    Preserve,
}

#[derive(clap::Parser)]
pub struct BossCliArgs {
    /// The source path, which will be synced to the destination path.
    /// Optionally contains a username and hostname for specifying remote paths.
    /// Format: [[username@]hostname:]path
    #[arg(required_unless_present="spec", conflicts_with="spec")]
    pub src: Option<RemotePathDesc>,
    /// The destination path, which will be synced from the source path.
    /// Optionally contains a username and hostname for specifying remote paths.
    /// Format: [[username@]hostname:]path
    #[arg(required_unless_present="spec", conflicts_with="spec")]
    pub dest: Option<RemotePathDesc>,

    /// Instead of specifying SRC and DEST, this can be used to perform a sync defined by a config file.
    #[arg(long)]
    pub spec: Option<String>,

    /// If set, forces redeployment of rjrssync to any remote targets, even if they already have an
    /// up-to-date copy.
    #[arg(long)]
    pub force_redeploy: bool,
    /// A list of filters that can be used to ignore some entries (files/folders) when performing the sync.
    /// Each filter is a regex, prepended with either a '+' or '-' character, to indicate if this filter
    /// should include or exclude matching entries.
    /// If the first filter provided is an include (+), then only those entries matching this filter will be included.
    /// If the first filter provided is an exclude (-), then only those entries not matching this filter will be included.
    /// Further filters can then add or remove entries.
    /// The regexes are matched against a normalized path relative to the root of the source/dest.
    /// Normalized means that forward slashes are always used as directory separators, never backwards slashes.
    /// If a folder does is excluded, then none of the contents of the folder will be seen, even if they would otherwise match.
    #[arg(name="filter", long, allow_hyphen_values(true))]
    pub filters: Vec<String>,
    /// Override the port used to connect to hostnames specified in src or dest.
    #[arg(long, default_value_t = 40129)]
    pub remote_port: u16,
    #[arg(value_enum, long, default_value_t=SymlinkMode::Unaware)]
    pub symlinks : SymlinkMode, //TODO: add this to spec file too

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

#[derive(Default, Debug, PartialEq)]
struct Spec {
    src_hostname: String,
    src_username: String,
    dest_hostname: String,
    dest_username: String,
    syncs: Vec<SyncSpec>,
}

#[derive(Default, Debug, PartialEq)]
struct SyncSpec {
    src: String,
    dest: String,
    filters: Vec<String>,
}

fn parse_string(yaml: &Yaml, key_name: &str) -> Result<String, String> {
    match yaml {
        Yaml::String(x) => Ok(x.to_string()),
        x => Err(format!("Unexpected value for '{}'. Expected a string, but got {:?}", key_name, x)),
    }
}

fn parse_sync_spec(yaml: &Yaml) -> Result<SyncSpec, String> {
    let mut result = SyncSpec::default();
    for (root_key, root_value) in yaml.as_hash().ok_or("Sync value must be a dictionary")? {
        match root_key {
            Yaml::String(x) if x == "src" => result.src = parse_string(root_value, "src")?,
            Yaml::String(x) if x == "dest" => result.dest = parse_string(root_value, "dest")?,
            Yaml::String(x) if x == "filters" => {
                match root_value {
                    Yaml::Array(array_yaml) => {
                        for element_yaml in array_yaml {
                            match element_yaml {
                                Yaml::String(x) => result.filters.push(x.to_string()),
                                x => return Err(format!("Unexpected value in 'filters' array. Expected string, but got {:?}", x)),
                            }
                        }
                    }
                    x => return Err(format!("Unexpected value for 'filters'. Expected an array, but got {:?}", x)),
                }
            },
            x => return Err(format!("Unexpected key in 'syncs' entry: {:?}", x)),
        }
    }

    if result.src.is_empty() {
        return Err("src must be provided and non-empty".to_string());
    }
    if result.dest.is_empty() {
        return Err("dest must be provided and non-empty".to_string());
    }

    Ok(result)
}

fn parse_spec_file(path: &Path) -> Result<Spec, String> {
    let mut result = Spec::default();

    let contents = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let docs = YamlLoader::load_from_str(&contents).map_err(|e| e.to_string())?;
    if docs.len() < 1 {
        // We allow >1 doc, but just ignore the rest, this might be useful for users, to use like a comments or versions
        return Err("Expected at least one YAML document".to_string());
    }
    let doc = &docs[0];

    for (root_key, root_value) in doc.as_hash().ok_or("Document root must be a dictionary")? {
        match root_key {
            Yaml::String(x) if x == "src_hostname" => result.src_hostname = parse_string(root_value, "src_hostname")?,
            Yaml::String(x) if x == "src_username" => result.src_username = parse_string(root_value, "src_username")?,
            Yaml::String(x) if x == "dest_hostname" => result.dest_hostname = parse_string(root_value, "dest_hostname")?,
            Yaml::String(x) if x == "dest_username" => result.dest_username = parse_string(root_value, "dest_username")?,
            Yaml::String(x) if x == "syncs" => {
                match root_value {
                    Yaml::Array(syncs_yaml) => {
                        for sync_yaml in syncs_yaml {
                            result.syncs.push(parse_sync_spec(sync_yaml)?);
                        }
                    }
                    x => return Err(format!("Unexpected value for 'syncs'. Expected an array, but got {:?}", x)),
                }
            },
            x => return Err(format!("Unexpected key in root dictionary: {:?}", x)),
        }
    }

    Ok(result)
}

pub fn boss_main() -> ExitCode {
    let args = BossCliArgs::parse();

    // Configure logging, based on the user's --quiet/--verbose flag.
    // If the RUST_LOG env var is set though then this overrides everything, as this is useful for developers
    let args_level = match (args.quiet, args.verbose) {
        (true, false) => "warn",
        (false, true) => "debug",
        (false, false) => "info",
        (true, true) => panic!("Shouldn't be allowed by cmd args parser"),
    };
    let mut builder = env_logger::Builder::from_env(Env::default().default_filter_or(args_level));
    builder.format(|buf, record| {
        let target_color = match record.target() {
            "rjrssync::boss" => Color::Rgb(255, 64, 255), //TODO: module has been renamed!
            "rjrssync::doer" => Color::Cyan,
            "remote doer" => Color::Yellow,
            _ => Color::Green,
        };
        let target_style = buf.style().set_color(target_color).clone();

        let level_style = buf.default_level_style(record.level());

        if record.level() == log::Level::Info {
            // Info messages are intended for the average user, so format them plainly
            //TODO: they should probably also be on stdout, not stderr as they are at the moment
            writeln!(
                buf,
                "{}",
                record.args()
            )
        } else {
            writeln!(
                buf,
                "{:5} | {}: {}",
                level_style.value(record.level()),
                target_style.value(record.target()),
                record.args()
            )
        }
    });
    builder.init();

    debug!("Running as boss");

    if args.symlinks != SymlinkMode::Unaware {
        error!("Symlink mode not supported yet!");
        return ExitCode::from(19);
    }

    // Decide what to sync - defined either on the command line or in a spec file if provided
    let mut spec = Spec::default();
    if let Some(s) = args.spec {
        spec = match parse_spec_file(Path::new(&s)) {
            Ok(s) => s,
            Err(e) => {
                error!("Failed to parse spec file at '{}': {}", s, e);
                return ExitCode::from(18)
            }
        }
    } else {
        let src = args.src.unwrap(); // Command-line parsing rules means these must be valid, if spec is not provided
        let dest = args.dest.unwrap();
        spec.src_hostname = src.hostname;
        spec.src_username = src.username;
        spec.dest_hostname = dest.hostname;
        spec.dest_username = dest.username;
        spec.syncs.push(SyncSpec { src: src.path, dest: dest.path, filters: args.filters });
    }

    // The src and/or dest may be on another computer. We need to run a copy of rjrssync on the remote
    // computer(s) and set up network commmunication.
    // There are therefore up to three copies of our program involved (although some may actually be the same as each other)
    //   Boss - this copy, which received the command line from the user
    //   Source - runs on the computer specified by the `src` command-line arg, and so if this is the local computer
    //            then this may be the same copy as the Boss. If it's remote then it will be a remote doer process.
    //   Dest - the computer specified by the `dest` command-line arg, and so if this is the local computer
    //          then this may be the same copy as the Boss. If it's remote then it will be a remote doer process.
    //          If Source and Dest are the same computer, they are still separate copies for simplicity.
    //          (It might be more efficient to just have one remote copy, but remember that there could be different users specified
    //           on the Source and Dest, with separate permissions to the paths being synced, so they can't access each others' paths,
    //           in which case we couldn't share a copy. Also might need to make it multithreaded on the other end to handle
    //           doing one command at the same time for each Source and Dest, which might be more complicated.)


    // Launch doers on remote hosts or threads on local targets and estabilish communication (check version etc.)
    let mut src_comms = match setup_comms(
        &spec.src_hostname,
        &spec.src_username,
        args.remote_port,
        "src".to_string(),
        args.force_redeploy,
    ) {
        Some(c) => c,
        None => return ExitCode::from(10),
    };
    let mut dest_comms = match setup_comms(
        &spec.dest_hostname,
        &spec.dest_username,
        args.remote_port,
        "dest".to_string(),
        args.force_redeploy,
    ) {
        Some(c) => c,
        None => return ExitCode::from(11),
    };

    // Perform the actual file sync(s)
    for sync_spec in &spec.syncs {
        // Parse the filter strings - check if they start with a + or a -
        let mut filters : Vec<Filter>  = vec![];
        for f in &sync_spec.filters {
            match f.chars().nth(0) {
                Some('+') => filters.push(Filter::Include(f.split_at(1).1.to_string())),
                Some('-') => filters.push(Filter::Exclude(f.split_at(1).1.to_string())),
                _ => {
                    error!("Invalid filter '{}': Must start with a '+' or '-'", f);
                    return ExitCode::from(18)
                }
            }
        }

        // Indicate which sync this is, if there are many
        if spec.syncs.len() > 1 {
            info!("{} => {}:", sync_spec.src, sync_spec.dest);
        }

        let sync_result = sync(&sync_spec.src, sync_spec.dest.clone(), &filters,
            args.dry_run, args.stats, &mut src_comms, &mut dest_comms);

        match sync_result {
            Ok(()) => (),
            Err(e) => {
                error!("Sync error: {}", e);
                return ExitCode::from(12);
            }
        }
    }

    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;
    use tempfile::NamedTempFile;

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

    #[test]
    fn test_parse_spec_file_missing() {
        let err = parse_spec_file(Path::new("does/not/exist")).unwrap_err();
        // Check for Windows and Linux error messages
        assert!(err.contains("cannot find the path") || err.contains("No such file"));
    }

    #[test]
    fn test_parse_spec_file_empty() {
        let s = NamedTempFile::new().unwrap();
        assert!(parse_spec_file(s.path()).unwrap_err().contains("Expected at least one YAML document"));
    }

    #[test]
    fn test_parse_spec_file_invalid_syntax() {
        let mut s = NamedTempFile::new().unwrap();
        writeln!(s, "!!").unwrap();
        assert!(parse_spec_file(s.path()).unwrap_err().contains("did not find expected tag"));
    }

    #[test]
    fn test_parse_spec_file_all_fields() {
        let mut s = NamedTempFile::new().unwrap();
        write!(s, r#"
            src_hostname: "computer1"
            src_username: "user1"
            dest_hostname: "computer2"
            dest_username: "user2"
            syncs:
            - src: T:\Source1
              dest: T:\Dest1
              filters: [ "-exclude1", "-exclude2" ]
            - src: T:\Source2
              dest: T:\Dest2
              filters: [ "-exclude3", "-exclude4" ]
        "#).unwrap();

        let expected_result = Spec {
            src_hostname: "computer1".to_string(),
            src_username: "user1".to_string(),
            dest_hostname: "computer2".to_string(),
            dest_username: "user2".to_string(),
            syncs: vec![
                SyncSpec {
                    src: "T:\\Source1".to_string(),
                    dest: "T:\\Dest1".to_string(),
                    filters: vec![ "-exclude1".to_string(), "-exclude2".to_string() ],
                },
                SyncSpec {
                    src: "T:\\Source2".to_string(),
                    dest: "T:\\Dest2".to_string(),
                    filters: vec![ "-exclude3".to_string(), "-exclude4".to_string() ],
                }
            ]
        };

        assert_eq!(parse_spec_file(s.path()), Ok(expected_result));
    }

    /// Checks that parse_spec_file() allows some fields to be omitted, with sensible defaults.
    #[test]
    fn test_parse_spec_file_default_fields() {
        let mut s = NamedTempFile::new().unwrap();
        write!(s, r#"
            syncs:
            - src: T:\Source1
              dest: T:\Dest1
        "#).unwrap();

        let expected_result = Spec {
            src_hostname: "".to_string(), // Default - not specified in the YAML
            src_username: "".to_string(), // Default - not specified in the YAML
            dest_hostname: "".to_string(), // Default - not specified in the YAML
            dest_username: "".to_string(), // Default - not specified in the YAML
            syncs: vec![
                SyncSpec {
                    src: "T:\\Source1".to_string(),
                    dest: "T:\\Dest1".to_string(),
                    filters: vec![], // Default - not specified in the YAML
                },
            ]
        };

        assert_eq!(parse_spec_file(s.path()), Ok(expected_result));
    }

    /// Checks that parse_spec_file() errors if required fields are omitted.
    #[test]
    fn test_parse_spec_file_missing_required_src() {
        let mut s = NamedTempFile::new().unwrap();
        write!(s, r#"
            syncs:
            - src: T:\Source1
        "#).unwrap();

        assert!(parse_spec_file(s.path()).unwrap_err().contains("dest must be provided and non-empty"));
    }

    /// Checks that parse_spec_file() errors if required fields are omitted.
    #[test]
    fn test_parse_spec_file_missing_required_dest() {
        let mut s = NamedTempFile::new().unwrap();
        write!(s, r#"
            syncs:
            - dest: T:\Dest1
        "#).unwrap();

        assert!(parse_spec_file(s.path()).unwrap_err().contains("src must be provided and non-empty"));
    }

    #[test]
    fn test_parse_spec_file_invalid_root() {
        let mut s = NamedTempFile::new().unwrap();
        write!(s, "123").unwrap();
        assert!(parse_spec_file(s.path()).unwrap_err().contains("Document root must be a dictionary"));
    }

    #[test]
    fn test_parse_spec_file_invalid_string_field() {
        let mut s = NamedTempFile::new().unwrap();
        write!(s, "dest_hostname: [ 341 ]").unwrap();
        assert!(parse_spec_file(s.path()).unwrap_err().contains("Unexpected value for 'dest_hostname'"));
    }

    #[test]
    fn test_parse_spec_file_invalid_field_name() {
        let mut s = NamedTempFile::new().unwrap();
        write!(s, "this-isnt-valid: 0").unwrap();
        assert!(parse_spec_file(s.path()).unwrap_err().contains("Unexpected key in root dictionary"));
    }

    #[test]
    fn test_parse_spec_file_invalid_syncs_field() {
        let mut s = NamedTempFile::new().unwrap();
        write!(s, "syncs: 0").unwrap();
        assert!(parse_spec_file(s.path()).unwrap_err().contains("Unexpected value for 'syncs'"));
    }

    #[test]
    fn test_parse_spec_file_invalid_sync_spec_type() {
        let mut s = NamedTempFile::new().unwrap();
        write!(s, r#"
            syncs:
            - not-a-dict
        "#).unwrap();
        assert!(parse_spec_file(s.path()).unwrap_err().contains("Sync value must be a dictionary"));
    }

    #[test]
    fn test_parse_spec_file_invalid_sync_spec_field() {
        let mut s = NamedTempFile::new().unwrap();
        write!(s, r#"
            syncs:
            - unexpected-field: 0
        "#).unwrap();
        assert!(parse_spec_file(s.path()).unwrap_err().contains("Unexpected key in 'syncs' entry"));
    }

    #[test]
    fn test_parse_spec_file_invalid_filters_type() {
        let mut s = NamedTempFile::new().unwrap();
        write!(s, r#"
            syncs:
            - filters: 0
        "#).unwrap();
        assert!(parse_spec_file(s.path()).unwrap_err().contains("Unexpected value for 'filters'"));
    }

    #[test]
    fn test_parse_spec_file_invalid_filters_element() {
        let mut s = NamedTempFile::new().unwrap();
        write!(s, r#"
            syncs:
            - filters: [ 9 ]
        "#).unwrap();
        assert!(parse_spec_file(s.path()).unwrap_err().contains("Unexpected value in 'filters' array"));
    }
}
