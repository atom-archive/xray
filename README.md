## Tasks
* [x] Update index from a depth-first traversal of an external file system
* [ ] Mirror remote operations to the local file system
* [ ] File system changes that happen in the middle of a scan
  * For example, if we scan a directory and then that same directory is moved later in the depth-first traversal before the scan completes, we would scan it again.
* [ ] Applying remote operations to an index that doesn't match the state of the file system
  * For example, a remote user adds a directory "b" inside a directory "a", but directory "a" is renamed to "c" before we can apply the result of the operation.
* [ ] Watch the file system
* [ ] Scan the file system
