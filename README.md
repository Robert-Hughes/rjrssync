Fast rsync-like tool for incrementally copying files. Runs natively on both Windows and Linux and uses network for communication, 
to maximise speed syncing between Windows and WSL filesystems (accessing WSL filesytem from Windows or vice-versa is slow).

TODO:

* Config file containing src/dest, ignore/allow list etc. Use serde_json?
    * List of folders to sync, with src and dest spec (computer and absolute path)
    * Each folder to be synced has list of include/exclude filters, applied in order (can mix and match include/exclude however you want)
    * Could have some kind of hierarchy of filters, so can exclude something without continuing to evaluate other filters?
    * Perhaps could have hard/soft includes/excludes - soft would keep evaluating other filters which may change the decision, hard would stop evaluating and keep that as the final decision.
    * Filters could be regexes on the path relative to the root (folder being synced)
    * If a dir is excluded by the filters (after resolving all filters), then we don't walk inside that dir, so stuff inside it will be excluded *even if the filters would have matched them*
* Security! We're opening ports that allow people to control our computers!
    * Could we use ssh for this somehow, like using ssh to forward our authenticate and then forward our connections?
    New plan is to not have a long-running daemon, but launch the process directly via ssh and keep the connection open during the transfer
    The remote instance would communicate via stdin and stdout, which is forwarded by SSH and so is secure and authenticated
    We can still do a version check, and then deploy new version if out-of-date. Try running /tmp/rjrssync and if it fails (doesn't exist)
    then deploy it and try again. If it does run successfully then we do the version handshake and if that fails we stop it and 
    deploy it and try again.
    Initiater would need to pipe stdin and out to its own, until the ssh auth is completed, at which point it can "take over" and issue
    commands to communicate (in binary).
    Remote process would could log to a file, as it can't use its stdout as that's used for communication. Stderr could be used though maybe and
    saved to a file on the initiator?
    Probably the initiator needs to handle the transfer (rather than orig plan of Src contacting Dest), as the hostname of Dest may not be the 
    same when addressing from Src (e.g. localhost). This is also more symmetrical/simpler?
* Network port specified as command line arg or other configuration variable?
* Investigate if parallelising some stages would speed it up, e.g. walking the dir structure on multiple threads, or sending data across network on multiple threads
* Investigate if pipelining some stages would speed it up, e.g. sending file list while also sending it