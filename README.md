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
* Network port specified as command line arg or other configuration variable?
* Investigate if parallelising some stages would speed it up, e.g. walking the dir structure on multiple threads, or sending data across network on multiple threads
* Investigate if pipelining some stages would speed it up, e.g. sending file list while also sending it