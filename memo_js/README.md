# Memo JS

This library enabling real-time collaborative coding by providing a conflict-free replicated state machine that models the state of a Git working tree. The core of the library is written in Rust for efficiency and reusability in other contexts. This library exposes the capabilities of the Rust core via WebAssembly and wraps them in an idiomatic JavaScript API.

## Initialize

Because WebAssembly compilation is asynchronous, to use this library, you must call the `init` function, which returns a promise. This function returns all remaining exports in an object.

```js
const memo = require("memo"); // alternatively: `import * as memo from "memo";`

async function main() {
  const { WorkTree } = await memo.init();
  // ... your code here
}
```

## Populate the work tree's base entries

In order to build a work tree, you need access to the Git repository on which the tree is based. After constructing a `WorkTree` with a non-zero replica id, you need to populate it with the paths from the Git commit on which this work tree is based via `appendBaseEntries`.

```js
const replicaId = 1;
const tree = new WorkTree(replicaId);
tree.appendBaseEntries([
  { depth: 1, name: "a", type: "Directory" },
  { depth: 2, name: "b", type: "File" },
  { depth: 1, name: "c", type: "File" },
])
```

The array of entries passed to `appendBaseEntries` should express a depth-first traversal of the directory hierarchy. When the depth of an entry increases by 1 from the previous entry, the entry is assumed to be the previous entry's child. For example, the entries passed above express the following paths:

```
a/
a/b
c
```

For now, Memo has no internal concept of commits. Application code will need to arrange for all replicas to build on top of the same commit state. When a participant wishes to commit, you'll need to coordinate building up a new work tree on top of the new state. We plan to handle more commit-related logic directly within the library in the future.

## Apply outstanding operations

If collaboration is already in progress, you can apply any outstanding operations with `applyOps`. We'll cover how these operations are generated below.

```js
tree.applyOps(operations)
```

## List the work tree's current entries

To list the work tree's current paths, call `entries`. This will return an array of entries arranged in a depth-first order, similar to the argument to `appendBaseEntries`. For example, the base entries populated above could be retrieved as follows:

```js
for entry of tree.entries() {
  console.log(entry.depth, entry.name, entry.type);
}

// Prints:
// 1 a Directory
// 2 b File
// 1 c File
```

Each returned entry has the following fields:

* `depth`: The length of the path leading to this entry.
* `name`: The entry's name.
* `fileId`: An opaque, base64 encoded binary value that can be passed to other methods on `WorkTree` to interact with this file.
* `type`: The type of this file (`"File"` or `"Directory"`)
* `status`: How this path has changed since the base commit (`"New"`, `"Renamed"`, `"Removed"`, `"Modified"`, or `"Unchanged"`)
* `visible`: Whether or not this file is currently visible (not deleted).

The `entries` method accepts two options.

* `showDeleted`: If `true`, returns entries for deleted files and directories, but marks them as `visible: false`.
* `descendInto`: An optional array of `FileId`s. If provided, the traversal will skip descending into any directory not present in this whitelist. You can use this option to limit the number of entries you need to process if you are rendering a UI with collapsed directories.
