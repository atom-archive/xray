# Memo JS

This library enables real-time collaborative coding by providing a conflict-free replicated state machine that models the state of a Git working tree. The core of the library is written in Rust for efficiency and reusability in other contexts. This library exposes the capabilities of the Rust core via WebAssembly and wraps them in an idiomatic JavaScript API.

## Initialize

Because WebAssembly compilation is asynchronous, to use this library, you must call the `init` function, which returns a promise. This function returns all remaining exports in an object.

```js
const memo = require("@atom/memo"); // alternatively: `import * as memo from "@atom/memo";`

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
  { depth: 2, name: "b", type: "Text" },
  { depth: 1, name: "c", type: "Text" }
])
```

The array of entries passed to `appendBaseEntries` should express a depth-first traversal of the directory hierarchy. When the depth of an entry increases by 1 from the previous entry, the entry is assumed to be the previous entry's child. For example, the entries passed above express the following paths:

```
a/
a/b
c
```

Since the application may need to perform I/O in order to fetch the base entries, you are free to start using the work tree before they are fully populated. You can also populate the base entries in a streaming fashion by calling `appendBaseEntries` multiple times and ensuring that the entries passed to each call pick up from the last entry passed in the previous call. If

For now, Memo has no internal concept of commits. Application code will need to arrange for all replicas to build on top of the same commit state and see the exact same base entries. When a participant wishes to commit, you'll need to coordinate building up a new work tree on top of the new state. We plan to handle more commit-related logic directly within the library in the future.

## Apply outstanding operations

If collaboration is already in progress, you can apply any outstanding operations with `applyOps`. We'll cover the details of generating and applying operations later.

```js
const fixupOps = tree.applyOps(operations)
// broadcast fixupOps to peers
```

If you have not finished populating the base entries via `appendBaseEntries`, some of these operations may be deferred until the entries that they reference are available.

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

The `entries` method accepts two options as fields in an optional object passed to the method.

* `showDeleted`: If `true`, returns entries for deleted files and directories, but marks them as `visible: false`.
* `descendInto`: An optional array of `FileId`s. If provided, the traversal will skip descending into any directory not present in this whitelist. You can use this option to limit the number of entries you need to process if you are rendering a UI with collapsed directories.

## Create new files and directories

File system operations on the work tree all function in terms of *file ids*, which are base64 encoded strings that can be obtained in various ways from the work tree. One way to access a file id is to create a new text file:

```js
const { fileId, operation } = tree.newTextFile();
// Send the operation to peers...
```

This returns a `fileId` and an `operation`, both of which are base64 encoded strings. The `operation` should be transmitted to all other collaborators and applied via `applyOps`, discussed in more detail later. After calling `newTextFile`, the created file exists in an "unsaved" form. To give it a name, pass the returned file id to the `rename` method.

```js
const operation = tree.rename(fileId, tree.getRootFileId(), "foo.txt");
// Send the operation to peers...
```

The `rename` method takes a file id to rename, the id of a parent directory, and the file's new name. In the example above, we access the id of the root directory via `getRootFileId()`. If you attempt to rename the file to a name that conflicts with an existing entry in the specified parent directory, an exception will be thrown.

Unlike files, directories cannot be created in a detached state. You'll need to specify a parent id and name at the time of creation.

```js
const { fileId, operation } = tree.createDirectory(tree.getRootFileId(), "a");
// Send the operation to peers...
```

## Renaming or deleting existing files

To rename or delete a file, you need access to its file id. You saw how to obtain the file id for a new file or directory above. There are a couple ways to obtain the id of an existing file. First, it's available in the `fileId` field in each of the objects returned by the `entries` method, described above. If you're rendering a UI based on this information, you could potentially associated this file id with a rendered UI element in some way. If you want to get the file id for a path, you can call `fileIdForPath`.

```js
const ops = []
const fileId1 = tree.fileIdForPath("a/b.txt");
const newParentId = tree.fileIdForPath("d/e");
ops.push(tree.rename(fileId1, newParentId, "b.txt"));
const fileId2 = tree.fileIdForPath("a/c.txt");
ops.push(tree.remove(fileId2));
// Send ops to peers...
```

You can also get the path for any file id by calling `pathForFileId`.

```js
console.log(tree.pathForFileId(fileId1)); // => "d/e/b.txt"
```

## Working with text files

As a prerequisite to interacting with the contents of any text file in the work tree, you'll need to call `openTextFile` and supply the text file's id along with its *base content*, representing the state of the file in the work tree's underlying Git commit.

```js
const bufferId = tree.openTextFile(fileId1, "Hello, world!");
```

In the example above, the string `"Hello, world!"` represents the contents of the file as it existed in the tree's base commit. If the file did not exist or was empty, you can pass an empty string. Again, just like with base entries, it's imperative that you supply the same content for all files on all replicas by ensuring that all work tree's build on the same commit in application code.

Once you have obtained a buffer id, you can get the text of the file, which might be different than the base text you supplied due to the application of remote operations. Remember to pass a *buffer id* obtained via `openTextFile` rather than a raw file id.

```js
console.log(tree.getText(bufferId)); // ==> "Hello, wonderful world!"
```

To edit a text file, call `edit` with the buffer id, an array of ranges to replace, and the new text.

```js
const operation = tree.edit(
  bufferId,
  [{ start: { row: 0, column: 0 }, end: { row: 0, column: 16 } }],
  "cruel"
);
console.log(tree.getText(bufferId)); // ==> "Hello, cruel world!"
// Send the operation to peers...
```

To obtain a diff containing just the changes that occurred since a specific point in time, use the `getVersion` and `changesSince` methods.

```js
const startVersion = tree.getVersion();
tree.applyOps(remoteOperations);
console.log(tree.changesSince(bufferId, startVersion));
// => [{ start: { row: 0, column: 7 }, end: { row: 0, column: 12 }, text: "happy"}]
```

Each change in the returned diff has a `start` and `end` based on the current state of the document along with the text that was inserted. You can iterate these changes in order and use the supplied coordinates to apply them to another document.

## Working with operations

All methods that update the state of the tree return *operations*, and you'll need to transmit and apply these operations in order to synchronize with other replicas. To apply remote operations, use the `applyOps` method.

```js
const remoteOps = await receiveOps();
const fixupOps = tree.applyOps(remoteOps);
broadcastOps(fixupOps);
```

Whenever you call `applyOps`, there is a chance that additional "fixup" operations could be generated to deal with cycles and name conflicts in the tree. Be sure to broadcast these operations to peers to ensure convergence.

If you're integrating with a text editor, you should capture the work tree's version vector before applying remote operations, then obtain and apply any changes to open buffers via `changesSince` method. Here is a sketch of how this would work.

```js
function applyOps(tree, ops, openEditors) {
  // Perform these steps synchronously:
  const baseVersion = tree.getVersion();
  const fixupOps = tree.applyOps(ops);
  for editor of openEditors {
    applyChanges(editor, tree.changesSince(editor.bufferId, baseVersion));
  }

  // Broadcasting fixup ops can happen at any time:
  broadcastOps(fixupOps);
}
```

It's important that you don't allow any local edits to be performed in between applying remote operations and updating the state of local editors, since local edits could cause the results of `changesSince` to be invalid for the current local editor state.
