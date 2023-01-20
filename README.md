About
=====

Fast rsync-like tool for incrementally copying files. Runs natively on both Windows and Linux and uses network for communication, to maximise speed when syncing between Windows and WSL filesystems.

Installation
============

```
cargo install rjrssync
```

This will build and install rjrssync from source.

rjrssync embeds pre-built binaries for other platforms inside itself as part of the build, so you may need to add some additional targets using `rustup` to get a working build.

Usage
=====

A quick example:

```
rjrssync local-folder/ user@hostname:/remote/folder
```

rjrssync uses `ssh` to estabilish an initial connection to the remote host but then switches to its own protocol to maximize performance. The first time that a remote host is used, rjrssync will deploy a pre-built binary to the remote host, which will be launched whenever rjrssync connects to that host. You will be prompted before this deployment happens.

See `rjrssync --help` for more.
