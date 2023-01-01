TODO:
=====

Interface
----------

* Could have some kind of hierarchy of filters, so can exclude something without continuing to evaluate other filters?
* Perhaps could have hard/soft includes/excludes - soft would keep evaluating other filters which may change the decision, hard would stop evaluating and keep that as the final decision.
* If a dir is excluded by the filters (after resolving all filters), then we don't walk inside that dir, so stuff inside it will be excluded *even if the filters would have matched them*. Document and test this?
* Ctrl-C doesn't seem to work very well at stopping rjrssync when it's running
* Tidy up --help output - maybe we need a short and long version?
  - Things in the README or notes.md shouldn't be needed for a user as they won't necessarily have access to then. These would need to be in --help, so might need moving.
  - The README is displayed on crates.io though, so it's likely that a user would see this.
* The naming for the behaviour flags isn't great - too verbose and not clear enough?
* In the spec file, could allow some settings to be set at both per-sync level, and at the top level (which would then apply to all syncs, but allowing overrides)
* Decide if info! (and other) log messages should be on stdout or stderr
* When showing multiple prompts, could remember the selection from previous time the same prompt was shown and use that as the default for the next one?
* Maybe could make "Connecting" spinner actually spin, until the first message from ssh?


Remote launching
----------------

* Additional SSH options as command-line arguments (separate for source and dest?)
* SSH host key verification prompt doesn't echo the user's typing, but it does seem to work anyway
* Using temporary dir means that rebooting the remote will mean we have to rebuild from scratch (on Linux)
* We could first attempt to use an already-installed version of rjrssync (in the PATH), and only if this doesn't exist or is incompatible do we deploy/build from scratch?
* Sometimes see "ssh stderr: mesg: ttyname failed: Inappropriate ioctl for device" when deploying to remote (I think on 'F**A' platforms). Can we hide this using "-T" for example?
* Launching on a new system can take a while, even if cargo is already installed, and if cargo isn't installed, this is an extra step for the user. Pre-built binaries?
* The prompt messages don't account for --dry-run, so it will look like things are actually going to be deleted, when they're not

Syncing logic
-------------

* Compare and sync file permissions?
* Progress bar
  - when copying large files, the progress bar won't move. Maybe have a sub-bar per-file for large files? Or change the bar to be total bytes rather than total files?
  - hide progress bar for --dry-run?
  - hide progress bar for --quiet?
  - hide progress bar for --no-progress?
* What happens if src and dest both point to the same place?
   - Either directly, or via symlink(s)?
* --dry-run isn't honoured when creating dest ancestors! It should instead say that it _would_ create the ancestors.
* CreateDestAncestors doesn't honour any of the behaviour flags to make it non-destructive?
* When splitting large files, the optimum chunk size might vary, we could adjust this dynamically. Right now I just picked an arbitrary value which could possibly be improved a lot! Also the same buffer size this is used for both the filesystem read() buffer size, _and_ the size of data we send to the boss, _and_ the size of data written on the other doer. The same size might not be optimal for all of these!
* We could check the modified timestamp of symlinks, and use this to (potentially) raise an error/prompt if the dest one is newer. Currently we always overwrite as we never check the timestamp.

Performance
------------

* Investigate if parallelising copying/deleting would speed it up
* Investigate if pipelining some stages would speed it up, e.g. encrypting and serialization at same time
* Probably better to batch together File() Responses, to avoid overhead from sending loads of messages
* If launching two remote doers, then it would be quicker to run the two setup_comms in parallel
   - need to watch out for ssh prompts though - what if we get two of these in parallel!
* Could investigate using UDP or something else to reduce TCP overhead, possibly this could speed up the TCP connection time?
* Benchmarking with two remotes rather than just one
   - maybe create abstraction for "targets", which can be local, \\wsl$, remote windows etc.,
     and each test table is a pair of source and dest targets.
* Benchmarking with explicit clear of linux cahce beforehand: sudo bash -c "sync; echo 3 > /proc/sys/vm/drop_caches"
   - And the same for Windows, it seems to have some sort of caching too (faster second time) https://learn.microsoft.com/en-us/windows/win32/api/memoryapi/nf-memoryapi-setsystemfilecachesize

               unsafe {
                if winapi::um::memoryapi::SetSystemFileCacheSize(usize::MAX, usize::MAX, 0) != 1 {
                    panic!("SetSystemFileCacheSize failed: {}", winapi::um::errhandlingapi::GetLastError());
                }
            }
   Also need the "memoryapi" feature for the winapi crate

   Need to grant the SE_INCREASE_QUOTA_NAME privilege, which it seems isn't as simple as running as admin
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
   - https://github.com/marketplace/actions/continuous-benchmark
   - Or do it ourselves using plotly, might be better? Can store data similarly to the above project, using a different branch of the repo, and maybe use github pages to view the rendered charts?
   - Make a note somewhere that because we're using WSL 1 on GHA, the "linux" filesystem performance
     won't be as good and might have "windows" characteristics (as the kernel is still windows)
* Keep looking for a way to get two github runners to talk to each other, so we can have one windows and one linux rather than having to use WSL which brings with it a bunch of problems. Maybe we can open a TCP tunnel between
two runners, some kind of NAT traversal handoff thing that doesn't involve all the traffic going through a third party, just the setup bits somehow?
* Various tests are leaving behind temporary folders, filling up with disk space! Especially benchmarks which are big!
* "The source/dest root is never checked against the filter - this is always considered as included." - test this (maybe already have a unit test actually!)
* Using tar for remote filesytem nodes messes about with symlinks when extracting on a different platform (Windows vs Linux)

Misc
-----

* On work PC this fails:
`cargo run D:\TempSource\ robhug01@localhost:/home/robhug01/TempDest -v`
ERROR | rjrssync::boss_frontend: Sync error: Unexpected response from dest GetEntries: Ok(Error("normalize_path failed: Illegal characters in path"))
* Warning if filter doesn't match anything, possibly after GetEntries but before actually doing anything (to prevent mistaken filter?)
* Would be nice to automatically detect cases where the version number hasn't been updated, e.g. if we could see that the Command/Response struct layout has changed.
* Document that ssh is used for connecting and launching, and that the sync is performed over a different network port, and that it is encrypted.
