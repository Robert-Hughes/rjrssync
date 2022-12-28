Notes on performance & security
===============================

Some perf results of walking directories on each OS:

```
   Host ->       Windows     Linux
Filesystem:
  Windows        100k        9k
   Linux          1k         500k
```

Conclusion => accessing WSL filesytem from Windows or vice-versa is slow, so we use native access.

Transferring data through the stdin/stdout of ssh is pretty slow (on Windows at least),
at most about 20MB/s. It's nice and secure though (we get authentication and encryption).

A faster option would be to use ssh's port forwarding so that we can use a separate TCP
connection. Testing on Windows showed this could peak at around 200MB/s. We lose some of the
security though, as anybody can connect to the tunnel.

The best for performance is a direct TCP connection (without ssh), which peaks at around 2GB/s
locally on Windows. We can use ssh for the initial setup, sharing some kind of secret key
so that each side of the connection is secure.

```

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

```

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

```

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

```

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

```

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

```

Benchmarking results
=======================

`cargo bench`, see benchmarks.rs. Run on both Windows and Linux.

Some more advanced options (see `cargo bench -- --help` for details):

`cargo bench -- --skip-setup --only-remote --programs rjrssync -n 5`

```
Each cell shows <min> - <max> over 5 sample(s) for: time | local memory (if available) | remote memory (if available)

Windows -> Windows
┌───────────────────┬────────────────────────────────────────────┐────────────────────────────────────────────┬────────────────────────────────────────────┬────────────────────────────────────────────┬───────────────────┐
│ Test case         │ rjrssync.exe (x10)                         │ scp (x5)                                   │ xcopy (x5)                                 │ robocopy (x5)                              │ APIs (x5)         │
├───────────────────┼────────────────────────────────────────────┤────────────────────────────────────────────┼────────────────────────────────────────────┼────────────────────────────────────────────┼───────────────────┤
│ Everything copied │ 2.95s   - 3.74s  | 33.34 MiB  - 78.08 MiB  │ 2.73s   - 3.38s  | 7.22 MiB   - 7.24 MiB   │ 2.41s   - 3.00s  | 5.75 MiB   - 6.00 MiB   │ 2.16s   - 2.39s  | 6.83 MiB   - 6.99 MiB   │ 1.97s   - 2.83s   │
│ Nothing copied    │ 83ms    - 147ms  | 9.04 MiB   - 10.27 MiB  │ Skipped                                    │ Skipped                                    │ 96ms    - 100ms  | 5.70 MiB   - 5.71 MiB   │ Skipped           │
│ Some copied       │ 98ms    - 173ms  | 9.07 MiB   - 10.22 MiB  │ Skipped                                    │ Skipped                                    │ 236ms   - 277ms  | 6.02 MiB   - 6.09 MiB   │ Skipped           │
│ Delete and copy   │ 2.83s   - 4.60s  | 64.78 MiB  - 65.18 MiB  │ Skipped                                    │ Skipped                                    │ 2.69s   - 3.21s  | 6.91 MiB   - 7.04 MiB   │ Skipped           │
│ Single large file │ 702ms   - 1.12s  | 34.23 MiB  - 54.46 MiB  │ 420ms   - 526ms  | 7.21 MiB   - 7.24 MiB   │ 385ms   - 423ms  | 5.63 MiB   - 5.65 MiB   │ 406ms   - 456ms  | 6.70 MiB   - 6.71 MiB   │ 373ms   - 389ms   │
└───────────────────┴────────────────────────────────────────────┘────────────────────────────────────────────┴────────────────────────────────────────────┴────────────────────────────────────────────┴───────────────────┘

Windows -> \\wsl$\...
┌───────────────────┬────────────────────────────────────────────┐────────────────────────────────────────────┬────────────────────────────────────────────┬────────────────────────────────────────────┬───────────────────┐
│ Test case         │ rjrssync.exe (x10)                         │ scp (x5)                                   │ xcopy (x5)                                 │ robocopy (x5)                              │ APIs (x5)         │
├───────────────────┼────────────────────────────────────────────┤────────────────────────────────────────────┼────────────────────────────────────────────┼────────────────────────────────────────────┼───────────────────┤
│ Everything copied │ 8.95s   - 10.94s | 78.43 MiB  - 79.31 MiB  │ 23.12s  - 23.84s | 7.23 MiB   - 7.24 MiB   │ 24.48s  - 29.05s | 12.30 MiB  - 12.57 MiB  │ 13.32s  - 14.80s | 13.75 MiB  - 13.80 MiB  │ 9.59s   - 10.96s  │
│ Nothing copied    │ 165ms   - 186ms  | 8.91 MiB   - 9.42 MiB   │ Skipped                                    │ Skipped                                    │ 891ms   - 1.18s  | 5.70 MiB   - 5.73 MiB   │ Skipped           │
│ Some copied       │ 225ms   - 271ms  | 9.00 MiB   - 9.37 MiB   │ Skipped                                    │ Skipped                                    │ 996ms   - 1.14s  | 5.93 MiB   - 5.97 MiB   │ Skipped           │
│ Delete and copy   │ 11.70s  - 14.22s | 65.15 MiB  - 65.48 MiB  │ Skipped                                    │ Skipped                                    │ 19.77s  - 22.74s | 13.84 MiB  - 14.09 MiB  │ Skipped           │
│ Single large file │ 6.71s   - 8.72s  | 222.19 MiB - 222.70 MiB │ 2.89s   - 2.94s  | 7.22 MiB   - 7.24 MiB   │ 2.89s   - 3.55s  | 12.18 MiB  - 12.19 MiB  │ 3.01s   - 3.30s  | 13.71 MiB  - 13.73 MiB  │ 2.82s   - 4.39s   │
└───────────────────┴────────────────────────────────────────────┘────────────────────────────────────────────┴────────────────────────────────────────────┴────────────────────────────────────────────┴───────────────────┘

Windows -> Remote Windows
┌───────────────────┬─────────────────────────────────────────────────────────────────────┐────────────────────────────────────────────┐
│ Test case         │ rjrssync.exe (x10)                                                  │ scp (x5)                                   │
├───────────────────┼─────────────────────────────────────────────────────────────────────┤────────────────────────────────────────────┤
│ Everything copied │ 3.51s   - 4.73s  | 20.01 MiB  - 21.18 MiB | 20.74 MiB  - 44.21 MiB  │ 5.46s   - 5.76s  | 7.96 MiB   - 8.15 MiB   │
│ Nothing copied    │ 337ms   - 768ms  | 8.71 MiB   - 9.48 MiB  | 7.04 MiB   - 7.57 MiB   │ Skipped                                    │
│ Some copied       │ 253ms   - 456ms  | 8.93 MiB   - 9.21 MiB  | 7.14 MiB   - 7.55 MiB   │ Skipped                                    │
│ Delete and copy   │ 3.18s   - 4.09s  | 21.64 MiB  - 22.41 MiB | 43.31 MiB  - 45.32 MiB  │ Skipped                                    │
│ Single large file │ 4.02s   - 5.17s  | 226.70 MiB - 227.45 MiB| 18.84 MiB  - 22.97 MiB  │ 5.51s   - 6.03s  | 7.48 MiB   - 7.50 MiB   │
└───────────────────┴─────────────────────────────────────────────────────────────────────┘────────────────────────────────────────────┘

Windows -> Remote Linux
┌───────────────────┬─────────────────────────────────────────────────────────────────────┐────────────────────────────────────────────┐
│ Test case         │ rjrssync.exe (x10)                                                  │ scp (x5)                                   │
├───────────────────┼─────────────────────────────────────────────────────────────────────┤────────────────────────────────────────────┤
│ Everything copied │ 1.07s   - 1.75s  | 20.83 MiB  - 21.25 MiB | 215.95 MiB - 215.95 MiB │ 11.12s  - 11.49s | 7.97 MiB   - 8.07 MiB   │
│ Nothing copied    │ 711ms   - 1.28s  | 8.67 MiB   - 9.45 MiB  | 680.20 MiB - 744.20 MiB │ Skipped                                    │
│ Some copied       │ 695ms   - 1.31s  | 9.03 MiB   - 9.52 MiB  | 744.07 MiB - 744.20 MiB │ Skipped                                    │
│ Delete and copy   │ 1.12s   - 1.70s  | 21.35 MiB  - 24.96 MiB | 692.12 MiB - 744.20 MiB │ Skipped                                    │
│ Single large file │ 3.36s   - 4.10s  | 227.21 MiB - 227.32 MiB| 215.95 MiB - 215.95 MiB │ 6.34s   - 7.21s  | 7.48 MiB   - 7.50 MiB   │
└───────────────────┴─────────────────────────────────────────────────────────────────────┘────────────────────────────────────────────┘

Linux -> Linux
┌───────────────────┬────────────────────────────────────────────┐───────────────────┬───────────────────┬───────────────────┬───────────────────┐
│ Test case         │ rjrssync (x10)                             │ rsync (x5)        │ scp (x5)          │ cp (x5)           │ APIs (x5)         │
├───────────────────┼────────────────────────────────────────────┤───────────────────┼───────────────────┼───────────────────┼───────────────────┤
│ Everything copied │ 188ms   - 760ms  | 756.81 MiB - 810.11 MiB │ 189ms   - 198ms   │ 90ms    - 97ms    │ 87ms    - 91ms    │ 94ms    - 109ms   │
│ Nothing copied    │ 145ms   - 227ms  | 1.25 GiB   - 1.31 GiB   │ 25ms    - 31ms    │ Skipped           │ Skipped           │ Skipped           │
│ Some copied       │ 359ms   - 1.15s  | 1.26 GiB   - 1.37 GiB   │ 32ms    - 36ms    │ Skipped           │ Skipped           │ Skipped           │
│ Delete and copy   │ 234ms   - 925ms  | 1.26 GiB   - 1.31 GiB   │ 244ms   - 259ms   │ Skipped           │ Skipped           │ Skipped           │
│ Single large file │ 517ms   - 1.01s  | 479.99 MiB - 612.00 MiB │ 1.87s   - 1.92s   │ 427ms   - 462ms   │ 424ms   - 487ms   │ 383ms   - 450ms   │
└───────────────────┴────────────────────────────────────────────┘───────────────────┴───────────────────┴───────────────────┴───────────────────┘

Linux -> /mnt/...
┌───────────────────┬────────────────────────────────────────────┐───────────────────┬───────────────────┬───────────────────┬───────────────────┐
│ Test case         │ rjrssync (x10)                             │ rsync (x5)        │ scp (x5)          │ cp (x5)           │ APIs (x5)         │
├───────────────────┼────────────────────────────────────────────┤───────────────────┼───────────────────┼───────────────────┼───────────────────┤
│ Everything copied │ 15.70s  - 17.94s | 779.57 MiB - 810.11 MiB │ 42.95s  - 44.38s  │ 16.87s  - 17.87s  │ 16.89s  - 17.42s  │ 17.31s  - 18.15s  │
│ Nothing copied    │ 15.60s  - 18.97s | 1.28 GiB   - 1.31 GiB   │ 23.58s  - 26.28s  │ Skipped           │ Skipped           │ Skipped           │
│ Some copied       │ 16.30s  - 17.93s | 1.28 GiB   - 1.31 GiB   │ 24.71s  - 26.56s  │ Skipped           │ Skipped           │ Skipped           │
│ Delete and copy   │ 28.26s  - 33.21s | 1.28 GiB   - 1.31 GiB   │ 51.36s  - 53.47s  │ Skipped           │ Skipped           │ Skipped           │
│ Single large file │ 3.74s   - 5.04s  | 542.05 MiB - 640.09 MiB │ 5.29s   - 6.35s   │ 3.82s   - 5.36s   │ 4.96s   - 5.02s   │ 5.35s   - 5.78s   │
└───────────────────┴────────────────────────────────────────────┘───────────────────┴───────────────────┴───────────────────┴───────────────────┘

Linux -> Remote Windows
┌───────────────────┬──────────────────────────────────────────────────────────────────────┐───────────────────┐
│ Test case         │ rjrssync (x10)                                                       │ scp (x5)          │
├───────────────────┼──────────────────────────────────────────────────────────────────────┤───────────────────┤
│ Everything copied │ 3.59s   - 6.94s  | 890.80 MiB - 942.14 MiB| 44.20 MiB  - 54.29 MiB   │ 5.67s   - 6.25s   │
│ Nothing copied    │ 461ms   - 1.27s  | 879.51 MiB - 942.14 MiB| 6.83 MiB   - 7.54 MiB    │ Skipped           │
│ Some copied       │ 3.37s   - 5.23s  | 891.57 MiB - 1006.14 MiB| 41.97 MiB  - 50.97 MiB  │ Skipped           │
│ Delete and copy   │ 4.32s   - 5.16s  | 891.63 MiB - 942.14 MiB| 54.82 MiB  - 56.74 MiB   │ Skipped           │
│ Single large file │ 3.04s   - 8.22s  | 680.03 MiB - 792.08 MiB| 18.84 MiB  - 26.86 MiB   │ 4.94s   - 5.20s   │
└───────────────────┴──────────────────────────────────────────────────────────────────────┘───────────────────┘

Linux -> Remote Linux
┌───────────────────┬─────────────────────────────────────────────────────────────────────┐───────────────────┬───────────────────┐
│ Test case         │ rjrssync (x10)                                                      │ rsync (x5)        │ scp (x5)          │
├───────────────────┼─────────────────────────────────────────────────────────────────────┤───────────────────┼───────────────────┤
│ Everything copied │ 343ms   - 651ms  | 890.81 MiB - 942.14 MiB| 215.95 MiB - 215.95 MiB │ 580ms   - 645ms   │ 1.77s   - 1.80s   │
│ Nothing copied    │ 109ms   - 319ms  | 879.59 MiB - 942.14 MiB| 680.20 MiB - 744.20 MiB │ 322ms   - 350ms   │ Skipped           │
│ Some copied       │ 344ms   - 882ms  | 891.58 MiB - 1.05 GiB  | 692.12 MiB - 808.20 MiB │ 346ms   - 371ms   │ Skipped           │
│ Delete and copy   │ 370ms   - 812ms  | 891.62 MiB - 942.29 MiB| 695.12 MiB - 744.20 MiB │ 647ms   - 743ms   │ Skipped           │
│ Single large file │ 1.85s   - 2.89s  | 616.03 MiB - 742.03 MiB| 215.95 MiB - 223.22 MiB │ 4.09s   - 4.21s   │ 4.55s   - 4.73s   │
└───────────────────┴─────────────────────────────────────────────────────────────────────┘───────────────────┴───────────────────┘

```

Notes on filters
================

Currently, the same filter is applied on both source and dest sides and there is no way to have a different filter on each side. This is simpler, but means that if you run a sync which copies some files you forgot to exclude, then add the exclude and re-run the sync, those files will still be present on the dest (but just hidden by the filter). So you would need to manually remove them which isn't great. If we allowed separate source/dest filters, then you could exclude the files just on the source and then they would be removed from the dest. However, having separate filters could lead to other potential issues - if you exclude some files on the dest only, and those files do exist on the source, then they will be copied every time regardless. Perhaps files should only be excludable on the source, or on both, but never just on the dest? Or perhaps a file should never be copied to the dest, if it would be excluded by the dest filter?