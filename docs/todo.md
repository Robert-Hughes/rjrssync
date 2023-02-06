TODO:
=====

Current
-------

* "Connecting" phase seems to be taking longer, especially on work PC. Only happens sometimes.  Could be because of mingw build? (vs. msvc build we were using before)
  - Maybe anti-virus scanning, because of embedded binary stuff?
  - Could it be static crt linking?
  - Or the mingw build?
  - Looks like it's being flagged up by windows defender, can see messages in Event Viewer. Possibly it was always scanning it, but because the binary size is so much bigger now it has a noticeable effect.

* Upload new version to crates.io

Interface
----------

* Could have some kind of hierarchy of filters, so can exclude something without continuing to evaluate other filters?
* Perhaps could have hard/soft includes/excludes - soft would keep evaluating other filters which may change the decision, hard would stop evaluating and keep that as the final decision.
* Ctrl-C doesn't seem to work very well at stopping rjrssync when it's running
* Documentation - Add reference somewhere to the trailing slash and symlink behaviour section of notes.md?
* In the spec file, could allow some settings to be set at both per-sync level, and at the top level (which would then apply to all syncs, but allowing overrides per-sync as well)
* Decide if info! (and other) log messages should be on stdout or stderr
* When showing multiple prompts, could remember the selection from previous time the same prompt was shown and use that as the default for the next one?
* Maybe could make "Connecting" spinner actually spin, until the first message from ssh?
* Long prompt messages (multi-line) duplicate themselves once answered.
* Could warn or similar when filters will lead to an error, like trying to delete a folder that isn't empty (because the filters hid the files inside)
* When prompting and given the choice to remember for "all occurences", we could show the number of occurences, e.g. "All occurences (17)".
* Option for force copying, even if it thinks it's up-to-date? Would this be just for files, or for folders etc. too?
* Document that ssh is used for connecting and launching, and that the sync is performed over a different network port, and that it is encrypted. Some of this added to readme already, but needs more. This should possibly be moved/copied to the --help so is available there too? Mention firewall issues?
* The "Connecting" spinner gets "lost" if we are deploying. it would be good to re-show this after deploy when we are trying to connect again (after Deploy successful!, there is a delay when nothing seems to be happening!)
* The progress bar update granularity (MARKER_THRESHOLD) should probably vary depending on the transfer speed? e.g. if it's 10MB that could be very quick or very long, depending on the connection etc.


Remote launching
----------------

* Additional SSH options as command-line arguments (separate for source and dest?)
* SSH host key verification prompt doesn't echo the user's typing, but it does seem to work anyway
* We could first attempt to use an already-installed version of rjrssync (in the PATH), and only if this doesn't exist or is incompatible do we deploy?
* Sometimes see "ssh stderr: mesg: ttyname failed: Inappropriate ioctl for device" when deploying to remote (I think on 'F**A' platforms). Can we hide this using "-T" for example?
* The prompt messages don't account for --dry-run, so it will look like things are actually going to be deleted, when they're not
* Embed Windows on Arm (aarch64-pc-windows-msvc) binary, and detect it when checking a remote OS
* When building embedded binaries, if the target platform cross-compiler isn't installed, then the build will produce a LOT of errors which is very noisy and slow. Maybe instead we should do our own quick check up front?
* Deploying a big binary to "less powerful"/slower targets may be bad because it will take ages to copy the big binary there, and the benefits of having a fully-functional rjrssync.exe on there may be minimal. Perhaps we do want the option(?) of deploying only a lite binary? That might make a lot of this work redundant, as we would no longer need to generate new big binaries on-demand, so wouldn't need to do all this section stuff. Perhaps instead we focus on making the binary smaller, which would be good anyway? One option could be to compress the embedded lite binaries.
* When the doer is listening on network port, if the boss never connects (e.g. due to firewall) it seems that even when you close the boss, the doer is left behind and doesn't close, possibly because it's just sat waiting for network connection that never comes (cos of firewall). Maybe we should have a timeout on the doer, if the boss doesn't connect within some short time, it should exit? Or if the stdin drops (i.e. ssh disappears)?
* Tidy up error reporting in boss_launch.rs

Syncing logic
-------------

* Compare and sync file permissions?
* Progress bar
  - hide progress bar for --dry-run? Confirm behaviour of all the progress code, as I think some is being skipped for dry run and some isn't (inconsistent)
  - hide progress bar for --quiet?
  - hide progress bar for --no-progress? This should be automatic when it's an "unattended" terminal?
  - show bytes or entries per seconds in the text as it goes?
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
* Looks like we're worse than competitors on wsl: Linux -> Linux for "everything copied"
* Looks like we're worse than competitors on windows: Windows -> Windows for "large file"


Testing
-------

* Test for --stats (maybe just all the command-line options...)
* Tests for when filesystem operations fail, e.g. failing to read/write a file
* Generating the benchmark graphs takes 8 mins! (On GitHub)
* Improve display of benchmark graph
   - add memory (local and remote) to the page somehow
   - Add moving average or similar to show trend, perhaps match it to a series of step functions?
   - add proper link(s) to the -order version
   - dynamic display, so can change stuff live on the page (e.g. swapping between timestamp and order, hiding/showing some platforms, looking at memory usage)
* Make a note somewhere that because we're using WSL 1 on GHA, the "linux" filesystem performance won't be as good and might have "windows" characteristics (as the kernel is still windows)
* Keep looking for a way to get two github runners to talk to each other, so we can have one windows and one linux rather than having to use WSL which brings with it a bunch of problems. Maybe we can open a TCP tunnel between two runners, some kind of NAT traversal handoff thing that doesn't involve all the traffic going through a third party, just the setup bits somehow?
   - https://en.wikipedia.org/wiki/NAT_traversal
   - https://github.com/ValdikSS/nat-traversal-github-actions-openvpn-wireguard/blob/master/README.md
* Confirm that github actions nightly schedule is working
* Various tests are leaving behind temporary folders, filling up with disk space! Especially benchmarks which are big!
* Using tar for remote filesytem nodes messes about with symlinks when extracting on a different platform (Windows vs Linux)
* Add test for multiple syncs with remote doer (to make sure it stays alive and can be used for multiple syncs) spec file
* Tests for progress bar (large files, small files, deleting and copying files). Could unit test some of the stuff, especially boss_progress.rs
* When installing rust on the GitHub job, could use the "minimal" profile to avoid downloading things like clippy, rust-docs etc. which we don't need
* Tests for --all-destructive-behaviour. Including that the individual settings can be overidden.


Misc
-----

* On work PC this fails:
`cargo run D:\TempSource\ robhug01@localhost:/home/robhug01/TempDest -v`
ERROR | rjrssync::boss_frontend: Sync error: Unexpected response from dest GetEntries: Ok(Error("normalize_path failed: Illegal characters in path"))
* Would be nice to automatically detect cases where the version number hasn't been updated, e.g. if we could see that the Command/Response struct layout has changed.
* Upload to cargo binstall (or similar) so that users don't need to build from source (especially if we're bundling embedded binaries, the initial build time will be looong!)
* Look at cargo dependency graph, to see if we can remove some dependencies
* Add Josh as crates.io package owner (needs to make an account first)
* Syncing something that doesn't exist to a remote, leads to dodgy looking errors:
ERROR: Sync error: src path 'something-that-doesnt-exist' doesn't exist!
ERROR: 2023-02-04T16:55:12.108217500Z Thread 'remote boss -> doer' failed with error: Error reading len: failed to fill whole buffer
ERROR: 2023-02-04T16:55:12.108269300Z Error sending final message: Error sending length: Connection reset by peer (os error 104)
* Link to perf figures from the README, to "prove" our perf advantages!