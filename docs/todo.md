TODO:
=====

Interface
----------

* Could have some kind of hierarchy of filters, so can exclude something without continuing to evaluate other filters?
* Perhaps could have hard/soft includes/excludes - soft would keep evaluating other filters which may change the decision, hard would stop evaluating and keep that as the final decision.
* If a dir is excluded by the filters (after resolving all filters), then we don't walk inside that dir, so stuff inside it will be excluded *even if the filters would have matched them*. Document this?
* --dry-run (and the same for -v) should make it clearer exactly what is being copied to where, e.g. give absolute paths. If there is a long path up to the root (or afterwards), could shorten it with ellipses, e.g. "Copying T:\work\...\bob\folder\...\thing.txt to X:\backups\...\newbackup\folder\...\thing.txt"
* Option to override the "dest file is newer" error. Test this behaviour (not sure if it's tested atm)
* Should filters expect to see trailing slashes on folder names or not? What about folder symlinks?

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

Syncing logic
-------------

* Compare and sync file permissions?
* Modified time:
    - need to account for time zone differences etc. between source and dest when updating the timestamp
    - would this play nicely with other tools (e.g. build systems) that check timestamps - it might think that it doesn't need to rebuild anything, as the new timestamp for this file is still really old?
    - Maybe instead we could store something else, like a hash or our own marker to indicate when this file was synced, so that the timestamp is "correct", but we know not to sync it again next time.
* Testing for sync logic, including between different combinations of windows and linux, remote and local etc.
   - Filters, and how they work on normalized paths between windows and linux.
   - Between different OSes,to make sure the path normalisation works
* Test for --dry-run
* Test for --stats (maybe just all the command-line options...)
* Progress bar
* What happens if src and dest both point to the same place?
   - Either directly, or via symlink(s)?
* --no-encryption option, might be faster?
* How to handle case when want to copy two different folders into the same destination, some sort of --no-delete?
* Use of SystemTime
   -  is this compatible between platforms, time zone changes, precision differences, etc. etc.
   - can we safely serialize this on one platform and deserialize on another?
* Consider warning for unexpected deletions (esp with replacing files with folders, see table in above section)
* --dry-run isn't honoured when creating dest ancestors! It should instead say that it _would_ create the ancestors.



Performance
------------

* Investigate if parallelising some stages would speed it up, e.g. walking the dir structure on multiple threads, or sending data across network on multiple threads
   - Could have one thread just doing filesystem calls to fill up a queue, and another thread processing those entries.
   - Maybe check if WalkDir is slow, by comparing its performance with direct std::fs stuff or even native OS stuff?
* Investigate if pipelining some stages would speed it up, e.g. sending file list while also sending it
* Probably better to batch together File() Responses, to avoid overhead from sending loads of messages
* Add to benchmark some remote tests (currently just testing local ones), and to/from WSL folders
   - Perhaps a separate table for local -> local, local -> WSL, local -> remote etc. etc.
   - Copy results into here (or similar), so can look at them without waiting for them to run
* Run benchmark tests on GitHub actions?
* Perf comparison with regular rsync (for cases where there are zero or few changes, and for cases with more changes)
* Performance tests, so we know if we've made something slower
  - Syncing large tree when nothing has changed
  - Copying single large file
  - Copying lots of small files
* If launching two remote doers, then it would be quicker to run the two setup_comms in parallel
* Could investigate using UDP or something else to reduce TCP overhead, possibly this could speed up the TCP connection time?



Misc
-----

* Make GitHub actions run on both Windows and Linux.
* Configure GitHub actions to run with remote hosts somehow
* piper and tcper maybe shouldn't be in the `bin/` folder, as then they count as part of the proper program,
but they should just be for testing/investigation. Maybe should be a separate crate?
* On work PC this fails:
`cargo run D:\TempSource\ robhug01@localhost:/home/robhug01/TempDest -v`
ERROR | rjrssync::boss_frontend: Sync error: Unexpected response from dest GetEntries: Ok(Error("normalize_path failed: Illegal characters in path"))
* Improve compile times. Is it the RustEmbed crate? Maybe the debug-embed feature of the crate could help?
* Maybe should extend test framework to support doing things remotely, like saving and loading filesystem nodes, making and clearing out a temporary folder etc.
* Upload to crates.io, so that we can "cargo install" from anywhere?
* "cargo install" should only install rjrssync, not the other binaries like piper etc.
* Warning if filter doesn't match anything, possibly after GetEntries but before actually doing anything (to prevent mistaken filter?)
* Running remote tests in parallel seems to cause hangs
* Would be nice to automatically detect cases where the version number hasn't been updated, e.g. if we 
could see that the Command/Response struct layout has changed.
* Turning profiling on/off results in incompatible network protocol, which isn't detected by the version number
* Something is messing up the line endings when running "cargo test" on windows. I replaced a bunch of ssh
commands with run_process_with_live_output to try to fix this, but it's still happening :( Possibly rjrssync itself is doing this?
* Investigate if Github Wiki would be useful for a lot of the stuff currently dumped in the README. Seems
like best bet is to make a docs/ subfolder
