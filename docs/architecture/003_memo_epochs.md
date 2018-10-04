The following document describes the sequence of operations that we should perform when the repository HEAD changes, both on the machine where the HEAD change occurred and at remote sites that receive the resulting epoch change.

The algorithms assume that version vectors don't reset across epochs. This does raise the concern that version vectors could grow without bound over the life of the repository, but we're going to suspend that concern temporarily to make progress.

### Creating a new epoch after HEAD moves

Assume we are currently at epoch A described by Tree T.

- Scan all the entries from Git's database based on the new HEAD into a new Tree T'.
- Synthesize and apply operations for all uncommitted changes via a `git diff`. This includes file system operations as well as uncommitted changes to file contents.
- For all buffers with unsaved edits in T:
  - Diff the last saved contents in T against the current contents of T' using the path of the buffer in T. This diff will describe a set of regions that have been touched outside of our control.
  - Go through each of the unsaved operations in T and check if they intersect with any of the regions in this diff to detect a conflict.
    - If there is a conflict, synthesize operations by performing a diff between the contents of T' and the contents of T and apply these as unsaved operations on top of T', then mark the buffer as in conflict.
    - Otherwise, transform all the unsaved operations according to the initial diff and apply them to the buffer in T'.

Afterward, we broadcast a new epoch B that contains the new HEAD SHA, the work tree's current version vector, a Lamport timestamp, and all synthesized operations.

### Receiving a new epoch

* Check Lamport timestamp of the epoch. If it's less than the current epoch's timestamp, ignore it. Otherwise, proceed to change the active epoch as follows:
  * Scan all entries from Git's database based on the new epoch's HEAD SHA into a new Tree T'.
  * Apply operations that are associated with the new epoch to T'.

What happens to buffers?
  * For all buffers containing edits not included in the epoch change's version vector:
    * If a file with the same path exists in T':
      * Diff the contents that are included in the version vector against the contents of T' using the path of the buffer in T. This diff will describe a set of regions that have been touched outside of our control.
      * Go through each of the local edits that were not part of the version vector. If they do not directly conflict with a region in the diff, synthesize a new operation with an adjusted position based on the diff and apply it to T'.
    * If no file with that path exists in T', we create it with initial contents from T.
