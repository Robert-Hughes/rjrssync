TODO:
=====

Current
-------



Interface
----------

* Could have some kind of hierarchy of filters, so can exclude something without continuing to evaluate other filters?
* Perhaps could have hard/soft includes/excludes - soft would keep evaluating other filters which may change the decision, hard would stop evaluating and keep that as the final decision.
* In the spec file, could allow some settings to be set at both per-sync level, and at the top level (which would then apply to all syncs, but allowing overrides per-sync as well)
* When showing multiple prompts, could remember the selection from previous time the same prompt was shown and use that as the default for the next one?
* Long prompt messages (multi-line) duplicate themselves once answered.
* Could warn or similar when filters will lead to an error, like trying to delete a folder that isn't empty (because the filters hid the files inside)
* When prompting and given the choice to remember for "all occurences", we could show the number of occurences, e.g. "All occurences (17)".
* The progress bar update granularity (MARKER_THRESHOLD) should probably vary depending on the transfer speed? e.g. if it's 10MB that could be very quick or very long, depending on the connection etc.
* Interrupt command (e.g. ctrl-something) which allows you to skip a file that's currently being copied, in case it's copying a big one that you don't want. Perhaps it shows a prompt, allowing you to skip that file or continue?

Remote launching
----------------

* Additional SSH options as command-line arguments (separate for source and dest?)
* SSH host key verification prompt doesn't echo the user's typing, but it does seem to work anyway
* We could first attempt to use an already-installed version of rjrssync (in the PATH), and only if this doesn't exist or is incompatible do we deploy?
* Sometimes see "ssh stderr: mesg: ttyname failed: Inappropriate ioctl for device" when deploying to remote (I think on 'F**A' platforms). Can we hide this using "-T" for example?
* The prompt messages don't account for --dry-run, so it will look like things are actually going to be deleted, when they're not
* Embed Windows on Arm (aarch64-pc-windows-msvc) binary, and detect it when checking a remote OS
* When building embedded binaries, if the target platform cross-compiler isn't installed, then the build will produce a LOT of errors which is very noisy and slow. Maybe instead we should do our own quick check up front?

Syncing logic
-------------

* Compare and sync file permissions?
* Progress bar - show bytes or entries per seconds in the text as it goes?
* We could check the modified timestamp of symlinks, and use this to (potentially) raise an error/prompt if the dest one is newer. Currently we always overwrite as we never check the timestamp.
* Now that we refactored the decision of what needs doing before we start doing it, it means that the --dry-run could maybe be implemented more simply by stopping after that decision stage, rather than passing it through everything

Performance
------------

* Investigate if parallelising copying/deleting would speed it up
* Investigate if pipelining some stages would speed it up, e.g. encrypting and serialization at same time
* Probably better to batch together EntryDetails Responses, to avoid overhead from sending loads of messages
* If launching two remote doers, then it would be quicker to run the two setup_comms in parallel
   - need to watch out for ssh prompts though - what if we get two of these in parallel!
* Could investigate using UDP or something else to reduce TCP overhead, possibly this could speed up the TCP connection time?
* Benchmarking with explicit clear of linux cache beforehand: sudo bash -c "sync; echo 3 > /proc/sys/vm/drop_caches"
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
* When splitting large files, the optimum chunk size might vary, we could adjust this dynamically. Right now I just picked an arbitrary value which could possibly be improved a lot! Also the same buffer size this is used for both the filesystem read() buffer size, _and_ the size of data we send to the boss, _and_ the size of data written on the other doer. The same size might not be optimal for all of these!

Testing
-------

* Improve display of benchmark graph
   - add memory (local and remote) to the page somehow
   - Add moving average or similar to show trend, perhaps match it to a series of step functions?
   - add proper link(s) to the -order version
   - dynamic display, so can change stuff live on the page (e.g. swapping between timestamp and order, hiding/showing some platforms, looking at memory usage)
* Keep looking for a way to get two github runners to talk to each other, so we can have one windows and one linux rather than having to use WSL which brings with it a bunch of problems. Maybe we can open a TCP tunnel between two runners, some kind of NAT traversal handoff thing that doesn't involve all the traffic going through a third party, just the setup bits somehow?
   - https://en.wikipedia.org/wiki/NAT_traversal
   - https://github.com/ValdikSS/nat-traversal-github-actions-openvpn-wireguard/blob/master/README.md
* Using tar for remote filesytem nodes messes about with symlinks when extracting on a different platform (Windows vs Linux)
* Using tar for remote filesytem nodes messes about with modified timestamps - they seem to get rounded. We've had to use files with explicit modified timestamps to workaround this for now.
* Tests for progress bar (large files, small files, deleting and copying files). Could unit test some of the stuff, especially boss_progress.rs
   * --no-progress
   * automatic no-progress when unattended terminal
   * --quiet mode
* When installing rust on the GitHub job, could use the "minimal" profile to avoid downloading things like clippy, rust-docs etc. which we don't need

Misc
-----

* Would be nice to automatically detect cases where the version number hasn't been updated, e.g. if we could see that the Command/Response struct layout has changed.
* Distribute binaries
 - also upload to crates.io?
 - upload musl build for linux, so it's more portable? This isn't the one we test though...
 - check works with cargo binstall
* Add Josh as crates.io package owner (needs to make an account first)
* Link to perf figures from the README, to "prove" our perf advantages!
