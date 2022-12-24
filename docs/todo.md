TODO:
=====

Interface
----------

* Could have some kind of hierarchy of filters, so can exclude something without continuing to evaluate other filters?
* Perhaps could have hard/soft includes/excludes - soft would keep evaluating other filters which may change the decision, hard would stop evaluating and keep that as the final decision.
* If a dir is excluded by the filters (after resolving all filters), then we don't walk inside that dir, so stuff inside it will be excluded *even if the filters would have matched them*. Document this?
* --dry-run (and the same for -v) should make it clearer exactly what is being copied to where, e.g. give absolute paths. If there is a long path up to the root (or afterwards), could shorten it with ellipses, e.g. "Copying T:\work\...\bob\folder\...\thing.txt to X:\backups\...\newbackup\folder\...\thing.txt"
* Ctrl-C doesn't seem to work very well at stopping rjrssync when it's running
* Tab-completion for parameters, part of clap?
* Tidy up --help output - maybe we need a short and long version?
  - Things in the README or notes.md shouldn't be needed for a user as they won't necessarily have access to then. These would need to be in --help, so might need moving.
* The naming for the behaviour flags isn't great - too verbose and not clear enough?
* In the spec file, could allow some settings to be set at both per-sync level, and at the top level (which would then apply to all syncs, but allowing overrides)
* Decide if info! (and other) log messages should be on stdout or stderr
* When showing multiple prompts, could remember the selection from previous time the same prompt was shown and use that as the default for the next one?


Remote launching
----------------

* Additional SSH options as command-line arguments (separate for source and dest?)
* SSH host key verification prompt doesn't echo the user's typing, but it does seem to work anyway
* Sometimes remote processes are left orphaned
* Using temporary dir means that rebooting the remote will mean we have to rebuild from scratch (on Linux)
* We could first attempt to use an already-installed version of rjrssync (in the PATH), and only if this
doesn't exist or is incompatible do we deploy/build from scratch?
* Sometimes see "ssh stderr: mesg: ttyname failed: Inappropriate ioctl for device" when deploying to remote (I think
on 'F**A' platforms). Can we hide this using "-T" for example?
* Launching on a new system can take a while, even if cargo is already installed, and if cargo isn't installed,
this is an extra step for the user. Pre-built binaries?
* The prompt messages don't account for --dry-run, so it will look like things are actually going to be deleted, when they're not

Syncing logic
-------------

* Compare and sync file permissions?
* Progress bar
  - can format the bar with number of bytes, or number of files, and it provides e.t.a. and rate of progress
  - when copying large files, the progress bar won't move. Maybe have a sub-bar per-file for large files? Or change
   the bar to be total bytes rather than total files?
  - hide progress bar for --dry-run?
  - hide progress bar for --quiet?
  - hide progress bar for --no-progress?
* What happens if src and dest both point to the same place?
   - Either directly, or via symlink(s)?
* --dry-run isn't honoured when creating dest ancestors! It should instead say that it _would_ create the ancestors.
* CreateDestAncestors doesn't honour any of the behaviour flags to make it non-destructive?
* When splitting large files, the optimum chunk size might vary, we could adjust this dynamically.
Right now I just picked an arbitrary value which could possibly be improved a lot!
Also the same buffer size this is used for both the filesystem read() buffer size, _and_ the size of data we send to the boss, _and_
// the size of data written on the other doer. The same size might not be optimal for all of these!
* We could check the modified timestamp of symlinks, and use this to (potentially) raise an error/prompt if the dest one is newer. Currently we always overwrite as we never check the timestamp.

Performance
------------

* Investigate if parallelising some stages would speed it up, e.g. walking the dir structure on multiple threads, or sending data across network on multiple threads
   - Maybe check if WalkDir is slow, by comparing its performance with direct std::fs stuff or even native OS stuff?
* Investigate if pipelining some stages would speed it up, e.g. encrypting and serialization at same time
* Probably better to batch together File() Responses, to avoid overhead from sending loads of messages
* If launching two remote doers, then it would be quicker to run the two setup_comms in paśallel
   - need to watch out for ssh prompts though - what if we get two of these in parallel!
* Could investigate using UDP or something else to reduce TCP overhead, possibly this could speed up the TCP connection time?
* Benchmarking with two remotes rather than just one
* Profiling events like send/receive could show the message type?
* --no-encryption option, might be faster?
   - Possibly want to keep the authentication aspects, but drop the encryption?
* Investigate different values of BOSS_DOER_CHANNEL_MEMORY_CAPACITY using profiling
   - could this be set based on e.g. 10% of the system memory?

Testing
-------

* Test for --dry-run
* Test for --stats (maybe just all the command-line options...)
* Tests for when filesystem operations fail, e.g. failing to read/write a file
* Clarify if filters should expect to see trailing slashes on folder names or not? What about folder symlinks? Tests + docs for this
* Run benchmark tests on GitHub actions?
* Various tests are leaving behind temporary folders, filling up with disk space!
* "The source/dest root is never checked against the filter - this is always considered as included." - test this (maybe already have a unit test actually!)

Misc
-----

* On work PC this fails:
`cargo run D:\TempSource\ robhug01@localhost:/home/robhug01/TempDest -v`
ERROR | rjrssync::boss_frontend: Sync error: Unexpected response from dest GetEntries: Ok(Error("normalize_path failed: Illegal characters in path"))
* Upload to crates.io, so that we can "cargo install" from anywhere?
* Warning if filter doesn't match anything, possibly after GetEntries but before actually doing anything (to prevent mistaken filter?)
* Would be nice to automatically detect cases where the version number hasn't been updated, e.g. if we
could see that the Command/Response struct layout has changed.
* Async comms might not be handling errors properly - the threads can stop early due to either the tcp connection or the channel being closed, and might need propagating somehow?
