# Memo: Operation-based version control

Memo is an experiment in a new approach to version control based on conflict-free replicated data types. Rather than tracking changes via snapshots of the working tree like Git does, Memo records and synchronizes changes in terms of individual operations to support the following:

* **Continuous persistence:** Changes are continuously and automatically persisted to the repository without the need to explicitly commit. Stream your edits to a server so that you or a teammate can seamlessly resume work on another machine later. Scrub and replay the edit history of any file back to the moment of its creation.

* **Live branches:** If you're connected to the network, the latest state of all branches is always visible and is updated in real time. Edits to different replicas of the same branch are automatically synchronized without the need for manual conflict resolution.

* **Persistent annotations:** Create a permalink to any piece of code that can always links to the same logical location in the latest version of the source code, even if it has been edited since the link was created.

* **Non-linear history:** Undo or redo changes in a specific selection. Group changes into arbitrary layers that can be dynamically toggled.

This project is currently in progress. Some of the functionality described in this document is complete, but much of it is still aspirational.

## Overview

When working with Git, you commit snapshots of your repository to a branch, then manually synchronize your copy of that branch with a remote replica by pulling and pushing commits. Memo branches are similar to Git branches, but they are automatically persisted on every edit and are continuously synchronized across all replicas in real-time without requiring manual conflict resolution.

Real-time change synchronization means that instead of waiting for changes to be committed and pushed to GitHub, cloud-based services can interact with the state of a repository as the code is being actively being written. For example, a service like Code Climate could perform incremental analysis on every branch of an Memo repository as it changes, inserting annotations into the repository that are be interpreted by client-side tooling. The ability for any replica to perform writes without risk of conflicts means that a cloud-based service could also perform edits such as code formatting.

Memo branches persist every change as it occurs, allowing a specific moments in the editing history to be identified with a version vector. This allows developers to write code for extended sessions without committing, then scrub the history to identify relevant checkpoints in their work after the fact. These checkpoints could also be identified automatically via analysis of the edit history. Fine-grained versioning means that any state of the code can be deployed into a development environment or a staging server without the ceremony of a commit. Just make some edits and click "play" to try them out.

Memo can be used as a standalone version control system, but it is also designed to interoperate smoothly with Git, meaning that an Memo repository can also be a Git repository. Memo branches are aware of the current Git branch, and Memo automatically maps Git commit SHAs to Memo version vectors as commits are created. If Memo detects that the user has checked out a different Git commit, it automatically updates the Memo branch to the appropriate version in the Memo history and replicates the check-out to all the branch's replicas.

## Memo's relationship to Xray

Our goal is to make Memo a standalone version control system that can be integrated with any scriptable text editor via an RPC connection to a local daemon process. Xray will import Memo as a library and build on its data structures directly to provide a first-class demonstration of its capabilities.

In the short term, we plan to focus solely on Memo's library interface so we can demo it via Xray. The standalone executable will come later, but we're developing it as a separate module from the beginning to ensure it's an easy change when the time comes.

## Conceptual model

### Operation-based representation

The repository is modeled as an operation-based conflict-free replicated data type (or CRDT) whose state is derived from a set of commutative operations.

#### Unique ids and version vectors

Globally-unique ids are a fundamental primitive of our data structures. Each replica is assigned a version 4 UUID, which is a randomly generated 128-bit integer. This is called the *replica id*. Each replica is then associated with an unsigned 64-bit counter which is used to produce *sequence numbers*. When a replica needs to generate a unique id, it combines its unique replica id with a sequence number generated via the counter.

Basing ids on an incrementing sequence number means that any two operations generated on a single replica can be ordered. To identify a specific moment in time given a set of operations generated on multiple replicas, we use a *version vector*. For every replica that produced an operation in the set, the version vector includes a mapping from that replica's id to a specific sequence number.

For example, lets use letters instead of 128-bit integers to represent replica ids. Imagine that replicas A, B, and C produce 10, 5, and 7 operations respectively. That means our operation set contains operations with ids A.1, A.2, A.3, ..., B.1, B.2.., C.1, ..., etc. We can identify a specific subset of these operations with the version vector {A: 7, B: 4, C: 5}. In practice, due to causal dependencies between operations, not all version vectors refer to valid repository states, but all valid repository states can be identified by a version vector.

#### Timelines

To support branching, the repository's operations are grouped into independent timelines. Each timeline is uniquely identified across all replicas, and every operation is associated with a single timeline on its initial broadcast. The state of a timeline evolves forward as operations are applied on one or more replicas. We can identify a specific *repository version* by combining a unique timeline id with a version vector describing a specific moment along that timeline.

When a repository is initially created, it has a single timeline. Additional timelines can be created that diverge from existing timelines. These additional timelines are associated with a *base version* that describes their starting point. All operations in the base version are assumed to be included in any version that references the new timeline's id.

#### Indexing operations

Operations represent the *essential state* of the repository, meaning that any repository version can always be derived from the set of operations it contains. However, to improve read performance, each timeline is associated with an indexed representation that allows the combined impact of its operations to be efficiently queried.

An timeline index is the combination of multiple fully-persistent data structures that represent the state of the file tree and the contents of every file within it. As operations flow into a replica, the index is updated in memory and the operations are temporarily persisted to disk. Periodically, a snapshot of the index is persisted to disk and the operations contained by the written version are deleted from storage.

The file system and individual text files are both indexed via persistent B-trees. Long-term storage is implemented with an off-the-shelf key-value database. Each node of our persistent B-tree is written to the database only once and assigned an identifier.

#### Indexing the working tree

Timelines are associated with a tree of files and directories that can be mirrored to the local file system. The state of this tree is represented via three persistent B-trees.

* `metadata` Contains an entry for each file and directory that records its type, inode, modified time, etc. Each entry is associated with a unique file id.
* `child_refs` Contains named entries representing the contents of all directories. It associates the file id of the containing directory with the name and id of its contents.
* `parent_refs` Associates each file or directory with the id of the directory that contains it. For directories, there is only one active parent ref at a time. Since files can be hard-linked, they may be associated with multiple parent refs at the same time.

When we detect a change on disk via platform-specific observer APIs, we generate operations by comparing the current contents of the repository with this index. We then send these operations to other replicas and use them to update their indices.

#### Indexing text files

If a directory tree node is associated with a text file, its contents are described by edit operations. Each insertion of text, be it a single character or a multi-kilobyte block, is associated with a unique identifier. Subsequently, that identifier can be combined with an offset to describe a unique position in the document. Deletions can be expressed as an (id, offset) describing the start and end of the deleted range along with a version vector expressing the subset of operations between these two points that were visible at the time the deletion was generated.

## Short term roadmap

* [ ] Core CRDT implementation:
  * [x] Achieve convergence for trees containing only directories in randomized mutation tests
  * [ ] Integrate support text files into directory trees
  * [ ] Achieve convergence for trees containing both files and directories in randomized mutation tests (including support for hard links)
  * [ ] Support for internal symbolic links
* [ ] Build a demo UI in Xray that includes the following features:
  * [ ] Automatically associate any GitHub repository with a Memo repository running on a demo server
  * [ ] Show all other memo branches for the current repository in a panel
  * [ ] Allow memo branches to be checked out and collaboratively edited
