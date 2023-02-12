About
=====

Fast rsync-like tool for incrementally copying files. Runs natively on both Windows and Linux and uses network for communication, to maximise speed when syncing between Windows and WSL filesystems.

Features
========

* Local or remote targets (including remote to remote)
* Fast, especially when nothing has changed
* Runs natively on Windows and Linux. Much faster than using WSL with `/mnt/` or `\\wsl$\`
* No setup needed on remote targets
* Preserves symlinks
* Filters
* Replay frequently used syncs
* Sync multiple folders in one command
* Dry run
* Progress bar and statistics

Installation
============

1. Install the rust build tools: https://www.rust-lang.org/tools/install
2. Run `cargo install rjrssync`

This will download the latest release of the source code from [crates.io](https://crates.io/crates/rjrssync), build and then install rjrssync.

## Supporting other platforms

This default build configuration will not include cross-compiled binaries for other platforms, and so rjrssync will not be able to sync to remote targets that are running different OSes or architectures. If you want to enable this feature, then some additional build steps are needed:

1. Install build tools for cross-compiling (see below)
2. Run `cargo install --feature embed-all rjrssync`

As part of this build, rjrssync is also cross-compiled for several other platforms and these are embedded into the final binary. You may need to set up your build environment for this to work, for example adding some additional targets to `rustup`:

### Example (Linux)

```
sudo apt install mingw-w64
rustup target add x86_64-pc-windows-gnu
rustup target add x86_64-unknown-linux-musl
rustup target add aarch64-unknown-linux-musl
```
### Example (Windows)

```
rustup target add x86_64-unknown-linux-musl
rustup target add aarch64-unknown-linux-musl
```

Usage
=====

A quick example:

```
rjrssync local-folder/ user@hostname:/remote/folder
```

rjrssync uses `ssh` to estabilish an initial connection to the remote host but then switches to its own protocol to maximize performance. The first time that a remote host is used, rjrssync will deploy a pre-built binary to the remote host, which will be launched whenever rjrssync connects to that host. You will be prompted before this deployment happens. rjrssync's protocol is encrypted and authenticated using [AES-GCM with a 128-bit key and 96-bit nonce](https://docs.rs/aes-gcm/latest/aes_gcm/index.html). It operates over TCP and so needs an open network port that the local copy can connect to the remote copy on. By default it automatically chooses a free port, but this can be overridden using `--remote-port`. You may need to adjust your firewall settings to allow this connection.

See `rjrssync --help` for more.

There are also some less well-presented notes on various features [here](docs/notes.md).