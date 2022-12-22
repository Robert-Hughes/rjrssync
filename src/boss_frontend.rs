use std::path::Path;
use std::process::ExitCode;
use std::io::Write;

use clap::{Parser, ValueEnum, CommandFactory};
use env_logger::{Env, fmt::Color};
use indicatif::ProgressBar;
use log::info;
use log::{debug, error};
use yaml_rust::{YamlLoader, Yaml};

use crate::profiling::{dump_all_profiling, start_timer, stop_timer, self};
use crate::{boss_launch::*, profile_this, function_name};
use crate::boss_sync::*;

#[derive(clap::Parser)]
pub struct BossCliArgs {
    /// The source path, which will be synced to the destination path.
    /// Optionally contains a username and hostname for specifying remote paths.
    /// Format: [[username@]hostname:]path
    #[arg(required_unless_present_any=["spec", "generate_auto_complete_script"], conflicts_with="spec")]
    pub src: Option<RemotePathDesc>,
    /// The destination path, which will be synced from the source path.
    /// Optionally contains a username and hostname for specifying remote paths.
    /// Format: [[username@]hostname:]path
    #[arg(required_unless_present_any=["spec", "generate_auto_complete_script"], conflicts_with="spec")]
    pub dest: Option<RemotePathDesc>,

    /// Instead of specifying SRC and DEST, this can be used to perform a sync defined by a config file.
    /// The spec file is a YAML file with the following structure:
    /// 
    /// ```
    ///     # Note that if no src_hostname is specified, then the respective src path is assumed to be local.
    ///     # The same goes for dest.
    ///     src_hostname: computer1
    ///     src_username: root
    ///     dest_hostname: computer2
    ///     dest_username: myuser
    ///     syncs:
    ///       - src: D:/Source
    ///         dest: D:/Dest
    ///         # Filters are regular expressions with a leading '+' or '-', indicating includes or excludes.
    ///         filter: [ "+.*\.txt", "-garbage\.txt" ]
    ///         dest_file_newer_behaviour: error
    ///         dest_file_older_behaviour: skip
    ///         dest_entry_needs_deleting_behaviour: prompt
    ///         dest_root_needs_deleting_behaviour: delete         
    ///       # Multiple paths can be synced
    ///       - src: D:/Source2
    ///         dest: D:/Dest2
    /// ```
    /// 
    /// In general, if parameters are provided in the both the spec file and then also as a command-line arg,
    /// the command-line arg will 'override' the value set in the spec file.
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
    /// The source/dest root is never checked against the filter - this is always considered as included.
    /// The regex must match the entire relative path for it to have an effect, not just part of it.
    #[arg(name="filter", long, allow_hyphen_values(true))]
    pub filters: Vec<String>,
    
    /// Overrides the TCP port that any remote copy(s) of rjrssync on hostnames specified in src or dest
    /// will listen on, used for network communication between the local and remote copies.
    /// If not specified, a free port will be chosen.
    #[arg(long)]
    pub remote_port: Option<u16>,

    #[arg(long)]
    pub dry_run: bool,

    /// Specifies behaviour when a file exists on both source and destination sides, but the 
    /// destination file has a newer modified timestamp. This might indicate that data is about
    /// to be unintentionally lost.
    /// The default is 'prompt'.
    // (the default isn't defined here, because it's defined in SyncSpec::default() and if we duplicate it
    //  here then we'll have no way of knowing if the user provided it on the cmd prompt as an override or not)
    #[arg(long,
        default_value_if("all_destructive_behaviour", "prompt", "prompt"),
        default_value_if("all_destructive_behaviour", "error", "error"),
        default_value_if("all_destructive_behaviour", "skip", "skip"),
        default_value_if("all_destructive_behaviour", "proceed", "overwrite"),
    )]
    pub dest_file_newer: Option<DestFileUpdateBehaviour>,

    /// Specifies behaviour when a file exists on both source and destination sides, but the 
    /// destination file has a older modified timestamp. This might indicate that data is about
    /// to be unintentionally lost.
    /// The default is 'overwrite'.
    // (the default isn't defined here, because it's defined in SyncSpec::default() and if we duplicate it
    //  here then we'll have no way of knowing if the user provided it on the cmd prompt as an override or not)
    #[arg(long, 
        default_value_if("all_destructive_behaviour", "prompt", "prompt"),
        default_value_if("all_destructive_behaviour", "error", "error"),
        default_value_if("all_destructive_behaviour", "skip", "skip"),
        default_value_if("all_destructive_behaviour", "proceed", "overwrite"),
    )]
    pub dest_file_older: Option<DestFileUpdateBehaviour>,

    /// Specifies behaviour when a file/folder/symlink on the destination side needs deleting.
    /// This might indicate that data is about to be unintentionally lost.
    /// The default is 'delete'.
    // (the default isn't defined here, because it's defined in SyncSpec::default() and if we duplicate it
    //  here then we'll have no way of knowing if the user provided it on the cmd prompt as an override or not)
    #[arg(long,
        default_value_if("all_destructive_behaviour", "prompt", "prompt"),
        default_value_if("all_destructive_behaviour", "error", "error"),
        default_value_if("all_destructive_behaviour", "skip", "skip"),
        default_value_if("all_destructive_behaviour", "proceed", "delete"),
    )]
    pub dest_entry_needs_deleting: Option<DestEntryNeedsDeletingBehaviour>,

    /// Specifies behaviour when the entire root on the destination side needs deleting.
    /// This might indicate that data is about to be unintentionally lost.
    /// This is separate to --dest-entry-needs-deleting, because there is some potentially
    /// surprising behaviour with regards to replacing the destination root that warrants
    /// special warning.
    /// The default is 'prompt'.
    // (the default isn't defined here, because it's defined in SyncSpec::default() and if we duplicate it
    //  here then we'll have no way of knowing if the user provided it on the cmd prompt as an override or not)
    #[arg(long,
        default_value_if("all_destructive_behaviour", "prompt", "prompt"),
        default_value_if("all_destructive_behaviour", "error", "error"),
        default_value_if("all_destructive_behaviour", "skip", "skip"),
        default_value_if("all_destructive_behaviour", "proceed", "delete"),
    )]
    pub dest_root_needs_deleting: Option<DestRootNeedsDeletingBehaviour>,

    /// Specifies behaviour when any destructive action is required.
    /// This might indicate that data is about to be unintentionally lost.
    /// This is a convenience for setting the following flags all to equivalant values:
    ///   --dest-file-newer
    ///   --dest-file-older
    ///   --dest-entry-needs-deleting
    ///   --dest-root-needs-deleting
    /// If any of those arguments are set individually, their value will be kept.
    /// This can be useful for running rjrssync in a "safe" mode (set this to 'prompt' or 'error'),
    /// or in an unattended "--yes" mode (set this to 'proceed').
    #[arg(long)]
    pub all_destructive_behaviour: Option<AllDestructiveBehaviour>,

    /// Outputs some additional statistics about the data copied.
    #[arg(long)]
    pub stats: bool, // This is a separate flag to --verbose, because that is more for debugging, but this is useful for normal users
    /// Hides all output except warnings and errors.
    #[arg(short, long, group="verbosity")]
    pub quiet: bool,
    /// Shows additional output.
    #[arg(short, long, group="verbosity")]
    pub verbose: bool,

    /// If provided, outputs an auto-complete script for the provided shell.
    /// For example, to configure auto-complete for bash:
    /// ```
    ///     rjrssync --generate-auto-complete-script=bash > /usr/share/bash-completion/completions/rjrssync.bash
    /// ```
    /// And for PowerShell:
    /// 
    /// Add the following line to the file "C:\Users\<USER>\Documents\WindowsPowerShell\Microsoft.PowerShell_profile.ps1"
    /// (create the file if it doesn't exist):
    /// ```
    ///     rjrssync --generate-auto-complete-script=powershell | Out-String | Invoke-Expression
    /// ```
    #[arg(long)]
    generate_auto_complete_script: Option<clap_complete::Shell>,

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

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, ValueEnum, Debug)]
pub enum DestFileUpdateBehaviour {
    /// The user will be asked what to do. (In a non-interactive environment, this is equivalent to 'error')
    Prompt,
    /// An error will be raised, the sync will stop and the destination file will not be overwritten.
    Error,
    /// The destination file will not be modified and the rest of the sync will continue.
    Skip,
    /// The destination file will be overwritten and the rest of the sync will continue.
    Overwrite,
}

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, ValueEnum, Debug)]
pub enum DestEntryNeedsDeletingBehaviour {
    /// The user will be asked what to do. (In a non-interactive environment, this is equivalent to 'error')
    Prompt,
    /// An error will be raised, the sync will stop and the destination file will not be deleted.
    Error,
    /// The destination file will not be deleted and the rest of the sync will continue.
    /// Note that this choice may lead to errors, as the entry that needed deleting might be preventing
    /// something else from being placed there.
    Skip,
    /// The destination entry will be deleted and the rest of the sync will continue.
    Delete,
}

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, ValueEnum, Debug)]
pub enum DestRootNeedsDeletingBehaviour {
    /// The user will be asked what to do. (In a non-interactive environment, this is equivalent to 'error')
    Prompt,
    /// An error will be raised, the sync will stop and the destination will not be changed.
    Error,
    /// The destination root will not be deleted and the sync will stop, but no error will be raised.
    /// The only difference between this and 'error' is that rjrssync will still report success, it just
    /// won't have actually done anything.
    Skip,
    /// The destination root will be deleted and the rest of the sync will continue.
    Delete,
}

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, ValueEnum, Debug)]
pub enum AllDestructiveBehaviour {
    /// The user will be asked what to do. (In a non-interactive environment, this is equivalent to 'error')
    Prompt,
    /// An error will be raised, the sync will stop and the destructive action will not take place.
    Error,
    /// The destructive action will not take place and the rest of the sync will continue, if possible.
    Skip,
    /// The destructive action will take place and the rest of the sync will continue.
    Proceed,
}

/// The hostname/usernames are fixed for the whole program (you can't set them differently for each
/// sync like you can with the filters etc.), because this doesn't bring much benefit over just 
/// running rjrssync multiple times with different arguments. We do allow syncing multiple folders
/// between the same two hosts though because this saves the connecting/setup time.
#[derive(Default, Debug, PartialEq)]
struct Spec {
    src_hostname: String,
    src_username: String,
    dest_hostname: String,
    dest_username: String,
    syncs: Vec<SyncSpec>,
}

#[derive(Debug, PartialEq)]
pub struct SyncSpec {
    pub src: String,
    pub dest: String,
    pub filters: Vec<String>,
    pub dest_file_newer_behaviour: DestFileUpdateBehaviour,
    pub dest_file_older_behaviour: DestFileUpdateBehaviour,
    pub dest_entry_needs_deleting_behaviour: DestEntryNeedsDeletingBehaviour,
    pub dest_root_needs_deleting_behaviour: DestRootNeedsDeletingBehaviour,
}
impl Default for SyncSpec {
    fn default() -> Self {
        Self { 
            src: String::new(),
            dest: String::new(),
            filters: vec![],
            dest_file_newer_behaviour: DestFileUpdateBehaviour::Prompt,
            dest_file_older_behaviour: DestFileUpdateBehaviour::Overwrite,
            dest_entry_needs_deleting_behaviour: DestEntryNeedsDeletingBehaviour::Delete,
            dest_root_needs_deleting_behaviour: DestRootNeedsDeletingBehaviour::Prompt, 
        }
    }
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
            Yaml::String(x) if x == "dest_file_newer_behaviour" => 
                result.dest_file_newer_behaviour = DestFileUpdateBehaviour::from_str(&parse_string(root_value, "dest")?, true)?,
            Yaml::String(x) if x == "dest_file_older_behaviour" => 
                result.dest_file_older_behaviour = DestFileUpdateBehaviour::from_str(&parse_string(root_value, "dest")?, true)?,
            Yaml::String(x) if x == "dest_entry_needs_deleting_behaviour" => 
                result.dest_entry_needs_deleting_behaviour = DestEntryNeedsDeletingBehaviour::from_str(&parse_string(root_value, "dest")?, true)?,
            Yaml::String(x) if x == "dest_root_needs_deleting_behaviour" => 
                result.dest_root_needs_deleting_behaviour = DestRootNeedsDeletingBehaviour::from_str(&parse_string(root_value, "dest")?, true)?,
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
    profile_this!();
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
    let timer = start_timer(function_name!());

    let args = {
        profile_this!("Parsing cmd line");
        BossCliArgs::parse()
    };

    if let Some(shell) = args.generate_auto_complete_script {
        let mut cmd = BossCliArgs::command();
        let name = cmd.get_name().to_string();
        clap_complete::generate(shell, &mut cmd, name, &mut std::io::stdout());
        return ExitCode::SUCCESS;
    }

    // Configure logging, based on the user's --quiet/--verbose flag.
    // If the RUST_LOG env var is set though then this overrides everything, as this is useful for developers
    {
        profile_this!("Configuring logging");
        let args_level = match (args.quiet, args.verbose) {
            (true, false) => "warn",
            (false, true) => "debug",
            (false, false) => "info",
            (true, true) => panic!("Shouldn't be allowed by cmd args parser"),
        };
        let mut builder = env_logger::Builder::from_env(Env::default().default_filter_or(args_level));
        builder.format(|buf, record| {
            // Strip "rjrssync::" prefix, as this doesn't add anything
            let target = record.target().replace("rjrssync::", "");
            let target_style = if target.contains("boss") {
                buf.style().set_color(Color::Rgb(255, 64, 255)).clone()
            } else if target.contains("remote") {
                buf.style().set_color(Color::Yellow).clone()
            } else if target.contains("doer") {
                buf.style().set_color(Color::Cyan).clone()
            } else {
                buf.style()
            };

            let level_style = buf.default_level_style(record.level());

            match record.level() {
                log::Level::Info => {
                    // Info messages are intended for the average user, so format them plainly
                    writeln!(
                        buf,
                        "{}",
                        record.args()
                    )
                }
                log::Level::Warn | log::Level::Error => {
                    // Warn/error messages are also for a regular user, but deserve a prefix indicating
                    // that they are an error/warning
                    writeln!(
                        buf,
                        "{}: {}",
                        level_style.value(record.level()),
                        record.args()
                    )
                }
                log::Level::Debug | log::Level::Trace => {
                    // Debug/trace messages are for developers or power-users, so have more detail
                    writeln!(
                        buf,
                        "{:5} | {}: {}",
                        level_style.value(record.level()),
                        target_style.value(target),
                        record.args()
                    )
                }
            }
        });
        builder.init();
    }

    debug!("Running as boss");

    // Decide what to sync - defined either on the command line or in a spec file if provided
    let spec = match resolve_spec(&args) {
        Ok(s) => s,
        Err(e) => {
            error!("{}", e);
            return ExitCode::from(18);
        }
    };

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

    let progress = ProgressBar::new_spinner().with_message("Connecting...");
    // Unfortunately we can't use enable_steady_tick to get a nice animation as we connect, because
    // this will clash with potential ssh output/prompts and potential output from the remote build 
    progress.tick(); 
    
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

    progress.finish_and_clear();

    // Perform the actual file sync(s)
    for sync_spec in &spec.syncs {
        // Indicate which sync this is, if there are many
        if spec.syncs.len() > 1 {
            info!("{} => {}:", sync_spec.src, sync_spec.dest);
        }

        let sync_result = sync(&sync_spec, args.dry_run, args.stats, &mut src_comms, &mut dest_comms);

        match sync_result {
            Ok(()) => (),
            Err(e) => {
                for e in e {
                    error!("Sync error: {}", e);
                }
                return ExitCode::from(12);
            }
        }
    }

    // Shutdown the comms before dumping profiling, so that any doer threads and comms threads have cleanly exited, 
    // and their profiling data is saved, and we have received profiling data from any remote doer processes.
    src_comms.shutdown();
    dest_comms.shutdown();

    stop_timer(timer);

    dump_all_profiling();

    // Dump memory usage figures when used for benchmarking. There isn't a good way of determining this from the benchmarking app
    // (especially for remote processes), so we instrument it instead.
    if std::env::var("RJRSSYNC_TEST_DUMP_MEMORY_USAGE").is_ok() {
        println!("Boss peak memory usage: {}", profiling::get_peak_memory_usage());
    }

    ExitCode::SUCCESS
}

/// Figures out the Spec that we should execute, from a combination of the command-line args
/// and a --spec file (if provided)
fn resolve_spec(args: &BossCliArgs) -> Result<Spec, String> {
    let mut spec = Spec::default();
    match &args.spec {
        Some(s) => {
            // If --spec was provided, use that as the starting point
            spec = match parse_spec_file(Path::new(&s)) {
                Ok(s) => s,
                Err(e) => return Err(format!("Failed to parse spec file at '{}': {}", s, e)),
            }
            // Some things in the spec file are overridable by command line equivalents (behaviours, filters etc.)
            // which is done below
        },
        None => {
            // No spec - the command-line must have the src and dest specified
            let src = args.src.as_ref().unwrap(); // Command-line parsing rules means these must be valid, if spec is not provided
            let dest = args.dest.as_ref().unwrap();
            spec.src_hostname = src.hostname.clone();
            spec.src_username = src.username.clone();
            spec.dest_hostname = dest.hostname.clone();
            spec.dest_username = dest.username.clone();
            spec.syncs.push(SyncSpec { 
                src: src.path.clone(), 
                dest: dest.path.clone(),
                ..Default::default()
            });
            // The rest of the command-line arguments are applied below (as they are also relevant
            // when a spec file is used).
        }
    }

    // Apply additional command-line args, which may override/augment what's in the spec file.
    for mut sync in &mut spec.syncs {
        if !args.filters.is_empty() {
            sync.filters = args.filters.clone();
        }
        if let Some(b) = args.dest_file_newer {
            sync.dest_file_newer_behaviour = b;
        }
        if let Some(b) = args.dest_file_older {
            sync.dest_file_older_behaviour = b;
        }
        if let Some(b) = args.dest_entry_needs_deleting {
            sync.dest_entry_needs_deleting_behaviour = b;
        }
        if let Some(b) = args.dest_root_needs_deleting {
            sync.dest_root_needs_deleting_behaviour = b;
        }
    }

    Ok(spec)
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
              dest_file_newer_behaviour: error
              dest_file_older_behaviour: skip
              dest_entry_needs_deleting_behaviour: prompt
              dest_root_needs_deleting_behaviour: delete         
            - src: T:\Source2
              dest: T:\Dest2
              filters: [ "-exclude3", "-exclude4" ]
              dest_file_newer_behaviour: prompt
              dest_file_older_behaviour: overwrite
              dest_entry_needs_deleting_behaviour: error
              dest_root_needs_deleting_behaviour: skip         
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
                    dest_file_newer_behaviour: DestFileUpdateBehaviour::Error,
                    dest_file_older_behaviour: DestFileUpdateBehaviour::Skip,
                    dest_entry_needs_deleting_behaviour: DestEntryNeedsDeletingBehaviour::Prompt,
                    dest_root_needs_deleting_behaviour: DestRootNeedsDeletingBehaviour::Delete,
                },
                SyncSpec {
                    src: "T:\\Source2".to_string(),
                    dest: "T:\\Dest2".to_string(),
                    filters: vec![ "-exclude3".to_string(), "-exclude4".to_string() ],
                    dest_file_newer_behaviour: DestFileUpdateBehaviour::Prompt,
                    dest_file_older_behaviour: DestFileUpdateBehaviour::Overwrite,
                    dest_entry_needs_deleting_behaviour: DestEntryNeedsDeletingBehaviour::Error,
                    dest_root_needs_deleting_behaviour: DestRootNeedsDeletingBehaviour::Skip,
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
                    ..Default::default()
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

    /// Checks that an invalid enum value for dest_file_newer_behaviour is rejected.
    /// We don't bother to test all the different behaviours in the same way, just this one.
    #[test]
    fn test_parse_spec_file_invalid_behaviour_value() {
        let mut s = NamedTempFile::new().unwrap();
        write!(s, r#"
            syncs:
            - dest_file_newer_behaviour: notallowed
        "#).unwrap();
        assert!(parse_spec_file(s.path()).unwrap_err().contains("Invalid variant: notallowed"));
    }

    /// Tests that command-line args can be used to override things set in the spec file.
    #[test]
    fn resolve_spec_overrides() {
        let mut spec_file = NamedTempFile::new().unwrap();
        write!(spec_file, r#"
            syncs:
            - src: a
              dest: b
              filters: [ +hello ]
              dest_file_newer_behaviour: skip
              dest_root_needs_deleting_behaviour: error
            - src: c
              dest: d

        "#).unwrap();

        let args = BossCliArgs::try_parse_from(&["rjrssync", 
            "--spec", spec_file.path().to_str().unwrap(),
            "--filter", "-meow",
            "--dest-file-newer", "error",
        ]).unwrap();
        let spec = resolve_spec(&args).unwrap();
        assert_eq!(spec, Spec { 
            syncs: vec![
                SyncSpec {
                    src: "a".to_string(),
                    dest: "b".to_string(),
                    filters: vec!["-meow".into()], // Overriden by command-line args
                    dest_file_newer_behaviour: DestFileUpdateBehaviour::Error,  // Overriden by command-line args
                    dest_root_needs_deleting_behaviour: DestRootNeedsDeletingBehaviour::Error, // From the spec file, not overriden by command-line args
                    ..Default::default()
                },
                SyncSpec {
                    src: "c".to_string(),
                    dest: "d".to_string(),
                    filters: vec!["-meow".into()], // Set by command-line args
                    dest_file_newer_behaviour: DestFileUpdateBehaviour::Error,  // Set by command-line args
                    ..Default::default()
                }
            ],
            ..Default::default() 
        });
    }

    /// Tests that --all-destructive-behaviour overrides things set in the spec file.
    #[test]
    fn all_destructive_behaviour_override() {
        let mut spec_file = NamedTempFile::new().unwrap();
        write!(spec_file, r#"
            syncs:
            - src: a
              dest: b
              dest_file_newer_behaviour: skip
              dest_root_needs_deleting_behaviour: prompt
            - src: c
              dest: d
        "#).unwrap();

        let args = BossCliArgs::try_parse_from(&["rjrssync", 
            "--spec", spec_file.path().to_str().unwrap(),
            "--all-destructive-behaviour", "error",
        ]).unwrap();
        let spec = resolve_spec(&args).unwrap();
        assert_eq!(spec, Spec { 
            syncs: vec![
                SyncSpec {
                    src: "a".to_string(),
                    dest: "b".to_string(),
                    // Overriden by command-line args
                    dest_file_newer_behaviour: DestFileUpdateBehaviour::Error,
                    dest_file_older_behaviour: DestFileUpdateBehaviour::Error,
                    dest_entry_needs_deleting_behaviour: DestEntryNeedsDeletingBehaviour::Error,
                    dest_root_needs_deleting_behaviour: DestRootNeedsDeletingBehaviour::Error,
                    ..Default::default()
                },
                SyncSpec {
                    src: "c".to_string(),
                    dest: "d".to_string(),
                    // Overriden by command-line args
                    dest_file_newer_behaviour: DestFileUpdateBehaviour::Error,
                    dest_file_older_behaviour: DestFileUpdateBehaviour::Error,
                    dest_entry_needs_deleting_behaviour: DestEntryNeedsDeletingBehaviour::Error,
                    dest_root_needs_deleting_behaviour: DestRootNeedsDeletingBehaviour::Error,
                    ..Default::default()
                }
            ],
            ..Default::default() 
        });
    }
}
