Notes on performance & security
===============================

Some perf results of walking directories on each OS:

   Host ->       Windows     Linux
Filesystem:
  Windows        100k        9k
   Linux          1k         500k

Conclusion => accessing WSL filesytem from Windows or vice-versa is slow, so we use native access.

Transferring data through the stdin/stdout of ssh is pretty slow (on Windows at least),
at most about 20MB/s. It's nice and secure though (we get authentication and encryption).

A faster option would be to use ssh's port forwarding so that we can use a separate TCP
connection. Testing on Windows showed this could peak at around 200MB/s. We lose some of the
security though, as anybody can connect to the tunnel.

The best for performance is a direct TCP connection (without ssh), which peaks at around 2GB/s
locally on Windows. We can use ssh for the initial setup, sharing some kind of secret key
so that each side of the connection is secure.

All readings from home PC (MANTA)

Stdin throughput (piper, buffers all 40960):
(no encryption)

Windows -> Windows (piper.exe | piper.exe): ~2.7GB/s
Windows -> WSL (piper.exe | wsl piper): ~500MB/s
WSL -> Windows (piper | piper.exe): 300-400MB/s
WSL -> WSL (piper | piper): ~7.5GB/s

SSH stdin throughput (piper, buffers all 40960):
(encrypted and authenticated etc, as it's all through SSH)

Windows -> Windows (piper.exe | ssh windows piper.exe): ~18MB/s
Windows -> WSL (piper.exe | ssh wsl piper): ~18MB/s
WSL -> Windows (piper | ssh windows piper.exe): ~200MB/s
WSL -> WSL (piper | ssh wsl piper.exe): 200-250MB/s

TCP connection throughput (tcper, buffers all 40960):
(no encryption)

Windows -> Windows: ~1.5GB/s (gets even faster with bigger buffers!)
Windows -> WSL: 500-600MB/s
WSL -> Windows: ~1.5GB/s (firewall needs enabling for PUBLIC networks!)
WSL -> WSL: ~6GB/s

SSH port forwarded throughput (tcper, buffers all 40960):
(note that even though the traffic is encrypted in-transit, anybody can connect to the ports!)

Windows -> Windows (ssh from Windows to Windows to forward local port): ~150MB/s
Windows -> WSL (ssh from Windows to WSL to forward local port): ~150MB/s
WSL -> Windows (ssh from WSL to Windows to forward local port): ~200MB/s
WSL -> WSL (ssh from WSL to WSL to forward local port): ~230MB/s

TCP connection with shared key encryption using serde_encrypt (tcper -e, buffers all 40960):
NOTE: these readings were taken with a bug in both piper and tcper where we weren't writing all the data! :O

Windows -> Windows: 250-300MB/s
Windows -> WSL: 250-300MB/s
WSL -> Windows: ~300MB/s
WSL -> WSL: ~300MB/s

TCP connection with shared key encryption using aes-gcm Aes256Gcm (tcper -e, buffers all 40960):

Windows -> Windows: ~700MB/s
Windows -> WSL: ~500MB/s
WSL -> Windows: 700-800MB/s
WSL -> WSL: 900-1000MB/s

Disabling PAM on sshd_config seems to speed up ssh login https://serverfault.com/questions/792486/ssh-connection-takes-forever-to-initiate-stuck-at-pledge-network


Tree representation
===================

There are some interesting design questions that arise about how to handle trees of files and folders,
for example whether two files should be considered equal if they have the same contents but different names.
If the answer is yes then it seems that the name shouldn't be part of the file itself, but is instead
used as an identifier _for_ the file. The same thing applies to folders. This seems to be the most
consistent view on the matter. This means that the program makes the destination object (file, folder etc.)
be the same as the source object, and that doesn't mean they need to have the same name.

There are several ambiguities about how to sync depending on whether the paths given exist, if they have a trailing
slash, etc or if they are a file or a folder. The goal is that the program should be intuitive and easy to use and
easy to reason about its behaviour. The current plan is to behave as follows:

Each row is for a different type of source that the source path points to (something that doesn't exist, an
existing file or an existing folder). Each column is for a different type of dest that the dest path points to.
Each row/col is further broken down into versions that include or do not include a trailing slash on the path.
The cell contents describe the behaviour given those inputs:
   - 'X' means this is an error
   - 'b' means that the source is copied over to the path b, creating, updating or replacing whatever might be there.
   - 'b/a' means that the source is copied over to the path b/a, creating, updating or replacing whatever might be there.
   - '!' indicates that the behaviour might be surprising/destructive because it deletes an existing file or folder and replaces it
        with a folder/file. We prompt the user for this.

|---------------------------------------------------------------------|
|          Dest ->    |  Non-existent |File or symlink|    Folder     |
|                     |---------------|---------------|---------------|
|  Source v           |   b   |  b/   |   b   |  b/ * |   b   |  b/   |
|---------------------|-------|-------|-------|-------|-------|-------|
|              src/a  |               |               |               |
| Non-existent        |       X       |       X       |       X       |
|              src/a/ |               |               |               |
|---------------------|-------|-------|-------|-------|-------|-------|
|              src/a  |   b   |  b/a  |   b   |   X   |   b!  |  b/a  |
| File or             |-------|-------|-------|-------|-------|-------|
| symlink     src/a/ *|   X   |   X   |   X   |   X   |   X   |   X   |
|---------------------|-------|-------|-------|-------|-------|-------|
|              src/a  |   b   |   b   |   b!  |   X   |   b   |   b   |
| Folder              |-------|-------|-------|-------|-------|-------|
|              src/a/ |   b   |   b   |   b!  |   X   |   b   |   b   |
|---------------------|-------|-------|-------|-------|-------|-------|

*: On Linux, a symlink with a trailing slash that points to a folder is treated as the target folder.

The behaviour can be summarised as a "golden rule" which is that after the sync, the object pointed to by the destination path will be identical to the object pointed to by the source path, i.e. `tree $SRC == tree $DEST`.

There is one exception, which is that if the dest path has a trailing slash, and source is an (existing) file, then the dest path is first modified to have the final part of the source path appended to it. e.g.:

`rjrssync folder/file.txt backup/` => `backup/file.txt`

This makes it more ergonomic to copy individual files. Unfortunately it makes the behaviour of files and folder inconsistent, but this this is fine because files and folders are indeed different, and it's worth the sacrifice.

It has the property that non-existent dest files/folders are treated the same as if they did exist, which means that you get a consistent final state no matter the starting state (behaviour is idempotent).

It also prevents unintended creation of nested folders with the same name which can be annoying, e.g.

`rjrssync src/folder dest/folder` => `dest/folder/...` (rather than `dest/folder/folder/...`)

Trailing slashes on files are always invalid, because this gives the impression that the file is actually a folder, and so could lead to unexpected behaviour.

Symlinks are treated the same as files, because that's essentially how rjrssync treats symlinks (it syncs
the link address, as if it was a small text file). Therefore they can't have trailing slashes.
Note that tab-completion (on bash and cmd at least) does not put a trailing slash on symlinks automatically,
so this shouldn't be a problem. (For folders, bash does do this which is why it's useful to allow trailing
slashes on folder names).

Note though that on Linux, the OS treats a trailing slash on a symlink to refer to the _target_
of the symlink, not the symlink itself, and so rjrssync doesn't actually see these as symlinks at all, so they
behave as folders.

Notes on symlinks
==================

Symlinks could be present as ancestors in the path(s) being synced (`a/b/symlink/c`),
the path being synced itself (`a/b/symlink`, one or both sides), or as one of the items inside a folder being synced.

Symlinks can point to either a file, a folder, nothing (broken), or another symlink, which itself could point to any of those.

Symlinks can cause cycles and DAGs, including pointing to itself. This shouldn't be too important for us,
as we (generally) never follow symlinks and so will never observe these cases.

On Windows, a symlink is either a "file symlink" or "directory symlink" (specified on creation),
whereas on Linux it is simply a symlink. A "directory symlink" that points to a file (possibly via other symlinks) or a "file symlink" that points to a directory is considered broken (similar to the target not existing at all).

It is possible to create an invalid symlink (target is the wrong 'type' or doesn't exist)

Symlink targets can be specified as relative or absolute.

Symlinks have their own modified time (which is when the link path was changed, not equal to the target's modified time), but we don't use as we can compare the link target instead (we don't have this luxury when
syncing files, because their contents might be huge, but a link target is small).

There can be multiple symlinks followed in a path being synced, e.g. <ROOT>/symlink1/folder2/symlink3/file,
but this would only be observed if rjrssync was unaware of symlinks (otherwise it would never walk into the first symlink), so we can't actually encounter this.

This crate has some useful explanation of some of these concepts: https://crates.io/crates/symlink.

The target link address on Windows might contain backslashes, which would need converting when sending over to Linux. The other way round should be fine, because Windows supports forward slashes too.

rjrssync treats symlinks as if they were simple text files containing their target address.
They are not (generally) followed or validated. They will be reproduced as accurately as possible on
the destination.
Note that this only applies to symlinks that are part of the sync (including as the root);
symlinks that are ancestors of the path provided as source or dest will always be followed in order
to get to the transfer root.

Known quirks with the current behaviour:

Existing windows symlink on the dest side, and then a broken Unix symlink on the source side, with the same target link address:
Currently this will result in an error, as we will attempt to create a Generic symlink on the dest, which will fail. In this case it's pretty likely that the user would prefer us to simply leave the dest symlink as it is
(whether it be a file or folder symlink).

Trailing slashes on symlinks are handled different on Windows and Linux, see above section on trailing slashes
for details.


It was considered for rjrssync to have two 'modes', as to whether it ignores the symlinks (treats them as
their targets) or syncs them as the links. However it was decided against this because of the increase in
complexity of the testing and some quirky behaviour in the "unaware" mode (see below):

Quirks of unaware mode:

On Linux, if there's a symlink to a folder and we're in unaware mode, then we won't be able to delete it because on Linux, deleting a symlink has to be done via remove_file, not remove_dir. However when in unaware mode, we see it as a dir, and so use remove_dir.

When deleting a symlink folder and we're in unaware model, the whole symlink *target* folder will be cleared out too, as well as deleting the symlink itself. This is somewhat surprising, but has to be the case because rjrssync is unaware that it is deleting stuff through a symlink.

Idea for filters, with re-usable "functions":
===============

"filters": [
   "src/.*" : include,
   "tests/.*" : include,
   ".*\.exe" : exclude,
   ".*/rob.exe" : include,
   "folderA/(.*)" : artifactsOnly($1),
   "folderB/(.*)" : artifactsOnly($1),
]

"artifactsOnly": [
   ".*" : exclude,
   "artifacts/.*\.bin": include,
   "other/artifacts/.*\.bin" : include,
]

Benchmarking results
=======================

`cargo bench`, see benchmarks.rs. Run on both Windows and Linux.

Some more advanced options (see `cargo bench -- --help` for details):

`cargo bench -- --skip-setup --only-remote --programs rjrssync -n 5`

TODO: update these with memory figures!!

Each cell shows <min> - <max> over 5 sample(s) for: time | local memory (if available) | remote memory (if available)

Windows -> Windows
┌───────────────────┬────────────────────────────────────────────┬────────────────────────────────────────────┬────────────────────────────────────────────┬────────────────────────────────────────────┬───────────────────┐
│ Test case         │ rjrssync.exe (x5)                          │ scp (x5)                                   │ xcopy (x5)                                 │ robocopy (x5)                              │ APIs (x5)         │
├───────────────────┼────────────────────────────────────────────┼────────────────────────────────────────────┼────────────────────────────────────────────┼────────────────────────────────────────────┼───────────────────┤
│ Everything copied │ 2.88s   - 3.19s  | 43.18 MiB  - 78.52 MiB  │ 2.36s   - 3.55s  | 7.23 MiB   - 7.23 MiB   │ 2.47s   - 2.94s  | 5.76 MiB   - 5.89 MiB   │ 1.88s   - 2.40s  | 6.85 MiB   - 6.96 MiB   │ 2.06s   - 2.64s   │
│ Nothing copied    │ 85ms    - 186ms  | 7.59 MiB   - 7.62 MiB   │ Skipped                                    │ Skipped                                    │ 94ms    - 117ms  | 5.70 MiB   - 5.71 MiB   │ Skipped           │
│ Some copied       │ 78ms    - 194ms  | 8.00 MiB   - 8.21 MiB   │ Skipped                                    │ Skipped                                    │ 237ms   - 259ms  | 6.03 MiB   - 6.05 MiB   │ Skipped           │
│ Delete and copy   │ 2.93s   - 3.44s  | 64.39 MiB  - 64.81 MiB  │ Skipped                                    │ Skipped                                    │ 2.67s   - 3.09s  | 6.93 MiB   - 7.04 MiB   │ Skipped           │
│ Single large file │ 629ms   - 794ms  | 29.10 MiB  - 121.99 MiB │ 423ms   - 441ms  | 7.22 MiB   - 7.23 MiB   │ 381ms   - 448ms  | 5.64 MiB   - 5.65 MiB   │ 385ms   - 421ms  | 6.70 MiB   - 6.71 MiB   │ 370ms   - 415ms   │
└───────────────────┴────────────────────────────────────────────┴────────────────────────────────────────────┴────────────────────────────────────────────┴────────────────────────────────────────────┴───────────────────┘

Windows -> \\wsl$\...
┌───────────────────┬────────────────────────────────────────────┬────────────────────────────────────────────┬────────────────────────────────────────────┬────────────────────────────────────────────┬───────────────────┐
│ Test case         │ rjrssync.exe (x5)                          │ scp (x5)                                   │ xcopy (x5)                                 │ robocopy (x5)                              │ APIs (x5)         │
├───────────────────┼────────────────────────────────────────────┼────────────────────────────────────────────┼────────────────────────────────────────────┼────────────────────────────────────────────┼───────────────────┤
│ Everything copied │ 8.74s   - 9.00s  | 78.49 MiB  - 78.80 MiB  │ 23.39s  - 24.05s | 7.21 MiB   - 7.24 MiB   │ 23.77s  - 28.95s | 12.27 MiB  - 12.57 MiB  │ 13.78s  - 16.66s | 13.76 MiB  - 13.88 MiB  │ 8.99s   - 14.19s  │
│ Nothing copied    │ 495ms   - 537ms  | 7.95 MiB   - 8.00 MiB   │ Skipped                                    │ Skipped                                    │ 925ms   - 1.78s  | 5.70 MiB   - 5.72 MiB   │ Skipped           │
│ Some copied       │ 489ms   - 622ms  | 8.48 MiB   - 8.58 MiB   │ Skipped                                    │ Skipped                                    │ 1.02s   - 2.15s  | 5.93 MiB   - 5.95 MiB   │ Skipped           │
│ Delete and copy   │ 11.31s  - 12.97s | 64.93 MiB  - 65.04 MiB  │ Skipped                                    │ Skipped                                    │ 18.77s  - 23.63s | 13.84 MiB  - 14.09 MiB  │ Skipped           │
│ Single large file │ 5.79s   - 6.24s  | 934.80 MiB - 938.82 MiB │ 2.90s   - 3.01s  | 7.22 MiB   - 7.23 MiB   │ 2.84s   - 3.35s  | 12.16 MiB  - 12.18 MiB  │ 2.86s   - 6.76s  | 13.62 MiB  - 13.68 MiB  │ 2.75s   - 4.00s   │
└───────────────────┴────────────────────────────────────────────┴────────────────────────────────────────────┴────────────────────────────────────────────┴────────────────────────────────────────────┴───────────────────┘

Windows -> Remote Windows
┌───────────────────┬─────────────────────────────────────────────────────────────────────┬────────────────────────────────────────────┐
│ Test case         │ rjrssync.exe (x5)                                                   │ scp (x5)                                   │
├───────────────────┼─────────────────────────────────────────────────────────────────────┼────────────────────────────────────────────┤
│ Everything copied │ 3.41s   - 4.15s  | 20.12 MiB  - 20.65 MiB | 30.43 MiB  - 44.86 MiB  │ 5.00s   - 7.84s  | 7.95 MiB   - 7.97 MiB   │
│ Nothing copied    │ 270ms   - 681ms  | 8.11 MiB   - 8.17 MiB  | 5.45 MiB   - 5.51 MiB   │ Skipped                                    │
│ Some copied       │ 281ms   - 543ms  | 8.39 MiB   - 8.63 MiB  | 5.70 MiB   - 5.78 MiB   │ Skipped                                    │
│ Delete and copy   │ 3.26s   - 4.22s  | 21.76 MiB  - 30.15 MiB | 42.81 MiB  - 43.08 MiB  │ Skipped                                    │
│ Single large file │ 3.65s   - 6.04s  | 819.02 MiB - 919.64 MiB| 18.77 MiB  - 30.78 MiB  │ 5.30s   - 7.81s  | 7.31 MiB   - 7.35 MiB   │
└───────────────────┴─────────────────────────────────────────────────────────────────────┴────────────────────────────────────────────┘

Windows -> Remote Linux
┌───────────────────┬─────────────────────────────────────────────────────────────────────┬────────────────────────────────────────────┐
│ Test case         │ rjrssync.exe (x5)                                                   │ scp (x5)                                   │
├───────────────────┼─────────────────────────────────────────────────────────────────────┼────────────────────────────────────────────┤
│ Everything copied │ 1.36s   - 1.99s  | 20.37 MiB  - 21.73 MiB | 215.60 MiB - 215.60 MiB │ 10.69s  - 14.28s | 7.95 MiB   - 8.01 MiB   │
│ Nothing copied    │ 995ms   - 1.60s  | 8.38 MiB   - 8.49 MiB  | 215.60 MiB - 215.60 MiB │ Skipped                                    │
│ Some copied       │ 939ms   - 1.02s  | 8.73 MiB   - 8.80 MiB  | 215.60 MiB - 215.60 MiB │ Skipped                                    │
│ Delete and copy   │ 1.32s   - 1.39s  | 21.79 MiB  - 23.04 MiB | 215.60 MiB - 215.60 MiB │ Skipped                                    │
│ Single large file │ 3.20s   - 4.15s  | 787.55 MiB - 803.02 MiB| 215.60 MiB - 215.60 MiB │ 5.80s   - 7.89s  | 7.33 MiB   - 7.36 MiB   │
└───────────────────┴─────────────────────────────────────────────────────────────────────┴────────────────────────────────────────────┘

Linux -> Linux
┌───────────────────┬────────────────────────────────────────────┬───────────────────┬───────────────────┬───────────────────┬───────────────────┐
│ Test case         │ rjrssync (x5)                              │ rsync (x5)        │ scp (x5)          │ cp (x5)           │ APIs (x5)         │
├───────────────────┼────────────────────────────────────────────┼───────────────────┼───────────────────┼───────────────────┼───────────────────┤
│ Everything copied │ 205ms   - 310ms  | 347.63 MiB - 347.63 MiB │ 191ms   - 234ms   │ 91ms    - 110ms   │ 93ms    - 119ms   │ 94ms    - 116ms   │
│ Nothing copied    │ 39ms    - 48ms   | 347.63 MiB - 347.63 MiB │ 24ms    - 33ms    │ Skipped           │ Skipped           │ Skipped           │
│ Some copied       │ 231ms   - 346ms  | 347.63 MiB - 347.63 MiB │ 32ms    - 40ms    │ Skipped           │ Skipped           │ Skipped           │
│ Delete and copy   │ 202ms   - 324ms  | 347.63 MiB - 347.63 MiB │ 246ms   - 266ms   │ Skipped           │ Skipped           │ Skipped           │
│ Single large file │ 527ms   - 1.55s  | 295.64 MiB - 859.62 MiB │ 1.95s   - 2.05s   │ 436ms   - 503ms   │ 436ms   - 527ms   │ 383ms   - 682ms   │
└───────────────────┴────────────────────────────────────────────┴───────────────────┴───────────────────┴───────────────────┴───────────────────┘

Linux -> /mnt/...
┌───────────────────┬────────────────────────────────────────────┬───────────────────┬───────────────────┬───────────────────┬───────────────────┐
│ Test case         │ rjrssync (x5)                              │ rsync (x5)        │ scp (x5)          │ cp (x5)           │ APIs (x5)         │
├───────────────────┼────────────────────────────────────────────┼───────────────────┼───────────────────┼───────────────────┼───────────────────┤
│ Everything copied │ 14.63s  - 18.65s | 347.63 MiB - 347.63 MiB │ 42.63s  - 61.86s  │ 16.58s  - 32.46s  │ 16.82s  - 25.63s  │ 18.10s  - 23.87s  │
│ Nothing copied    │ 15.13s  - 20.40s | 347.63 MiB - 347.63 MiB │ 24.49s  - 47.55s  │ Skipped           │ Skipped           │ Skipped           │
│ Some copied       │ 15.08s  - 16.85s | 347.63 MiB - 347.63 MiB │ 27.79s  - 34.88s  │ Skipped           │ Skipped           │ Skipped           │
│ Delete and copy   │ 27.73s  - 32.28s | 347.63 MiB - 347.63 MiB │ 52.38s  - 77.35s  │ Skipped           │ Skipped           │ Skipped           │
│ Single large file │ 4.01s   - 4.46s  | 1.16 GiB   - 1.27 GiB   │ 5.87s   - 11.31s  │ 4.85s   - 7.47s   │ 4.84s   - 6.27s   │ 5.40s   - 6.39s   │
└───────────────────┴────────────────────────────────────────────┴───────────────────┴───────────────────┴───────────────────┴───────────────────┘

Linux -> Remote Windows
┌───────────────────┬─────────────────────────────────────────────────────────────────────┬───────────────────┐
│ Test case         │ rjrssync (x5)                                                       │ scp (x5)          │
├───────────────────┼─────────────────────────────────────────────────────────────────────┼───────────────────┤
│ Everything copied │ 3.38s   - 4.05s  | 479.66 MiB - 479.66 MiB| 46.34 MiB  - 54.72 MiB  │ 5.48s   - 7.89s   │
│ Nothing copied    │ 546ms   - 1.09s  | 479.66 MiB - 479.66 MiB| 5.44 MiB   - 5.82 MiB   │ Skipped           │
│ Some copied       │ 3.22s   - 3.91s  | 479.66 MiB - 479.66 MiB| 35.23 MiB  - 53.81 MiB  │ Skipped           │
│ Delete and copy   │ 3.76s   - 5.62s  | 479.66 MiB - 479.66 MiB| 55.78 MiB  - 56.24 MiB  │ Skipped           │
│ Single large file │ 2.84s   - 3.29s  | 1.22 GiB   - 1.22 GiB  | 18.76 MiB  - 50.81 MiB  │ 4.75s   - 6.97s   │
└───────────────────┴─────────────────────────────────────────────────────────────────────┴───────────────────┘

Linux -> Remote Linux
┌───────────────────┬─────────────────────────────────────────────────────────────────────┬───────────────────┬───────────────────┐
│ Test case         │ rjrssync (x5)                                                       │ rsync (x5)        │ scp (x5)          │
├───────────────────┼─────────────────────────────────────────────────────────────────────┼───────────────────┼───────────────────┤
│ Everything copied │ 588ms   - 1.40s  | 479.66 MiB - 479.66 MiB| 215.60 MiB - 215.60 MiB │ 565ms   - 743ms   │ 1.71s   - 2.07s   │
│ Nothing copied    │ 328ms   - 611ms  | 479.66 MiB - 479.66 MiB| 215.60 MiB - 215.60 MiB │ 330ms   - 507ms   │ Skipped           │
│ Some copied       │ 589ms   - 934ms  | 479.66 MiB - 479.66 MiB| 215.60 MiB - 215.60 MiB │ 336ms   - 416ms   │ Skipped           │
│ Delete and copy   │ 690ms   - 1.29s  | 479.66 MiB - 479.66 MiB| 215.60 MiB - 215.60 MiB │ 638ms   - 730ms   │ Skipped           │
│ Single large file │ 1.81s   - 2.95s  | 1.09 GiB   - 1.22 GiB  | 215.60 MiB - 215.60 MiB │ 4.04s   - 5.68s   │ 4.37s   - 4.70s   │
└───────────────────┴─────────────────────────────────────────────────────────────────────┴───────────────────┴───────────────────┘

Notes on filters
================

Currently, the same filter is applied on both source and dest sides and there is no way to have a different filter on each side. This is simpler, but means that if you run a sync which copies some files you forgot to exclude, then add the exclude and re-run the sync, those files will still be present on the dest (but just hidden by the filter). So you would need to manually remove them which isn't great. If we allowed separate source/dest filters, then you could exclude the files just on the source and then they would be removed from the dest. However, having separate filters could lead to other potential issues - if you exclude some files on the dest only, and those files do exist on the source, then they will be copied every time regardless. Perhaps files should only be excludable on the source, or on both, but never just on the dest? Or perhaps a file should never be copied to the dest, if it would be excluded by the dest filter?