# Memo JS

This library enables real-time collaborative coding by providing a conflict-free replicated state machine that models the state of a Git working tree. The core of the library is written in Rust for efficiency and reusability in other contexts. This library exposes the capabilities of the Rust core via WebAssembly and wraps them in an idiomatic JavaScript API.

## Initialization

Because WebAssembly compilation is asynchronous, to use this library, you must call the `init` function, which returns a promise. This function returns all remaining exports in an object.

```ts
import * as memo from "@atom/memo";

async function main() {
  const { WorkTree } = await memo.init();
  // ...your code here...
}
```

## Creating a WorkTree

`WorkTree` is the fundamental abstraction provided by this library. To create one, you need to supply a (possibly `null`) base commit OID, an array of initial operations and a `GitProvider`:

```ts
const base = "8251a3c491b3884d7f828d2a1c5c565855171a2c";
const startOps = await fetchInitialOps();
const [tree, ops] = WorkTree.create(base, startOps, gitProvider);
broadcast(ops);
```

In the example above, `base` and `startOps` will be used in a mutually exclusive way. That is, if `startOps` is empty, the work tree will be automatically reset to `base`. Vice versa, the active base will be inferred based on the content of `startOps`. You can ignore how `fetchInitialOps` works for now, as it will be covered later in the guide.

The third required parameter, `gitProvider`, is an object that implements the `GitProvider` interface:

```ts
export interface GitProvider {
  baseEntries(oid: Oid): AsyncIterable<BaseEntry>;
  baseText(oid: Oid, path: Path): Promise<string>;
}

// This is a provider that reads Git data from github.com.
class GitHubProvider implements GitProvider {
  baseEntries(oid: Oid): AsyncIterable<BaseEntry> {
    const entries = await fetch(
      `/repos/rust-lang/rust/git/trees/${oid}?recursive=1"`
    );
    for (const entry of entries) {
      yield fromGitHubEntryToBaseEntry(entry);
    }
  }

  baseText(oid: Oid, path: Path): Promise<string> {
    const file = await fetch(
      `/repos/rust-lang/rust/contents/${path}?ref=${oid}`
    );
    return fromBase64ToString(file.content);
  }
}
```

The `baseEntries` method must return a collection that can be asynchronously iterated over and that yields `memo.BaseEntry` elements, like the following:

```ts
{ depth: 1, name: "a", type: memo.FileType.Directory }
{ depth: 2, name: "b.txt", type: memo.FileType.Text }
{ depth: 1, name: "c.txt", type: memo.FileType.Text }
```

## List the work tree's current entries

To list the work tree's current paths, call `entries`. This will return an array of entries arranged in a depth-first order, similar to the entries returned by `GitProvider.prototype.baseEntries`. For example, the base entries populated above could be retrieved as follows:

```ts
for (const entry of tree.entries()) {
  console.log(entry.depth, entry.name, entry.type);
}

// Prints:
// 1 a Directory
// 2 b.txt File
// 1 c.txt File
```

Each returned entry has the following fields:

- `depth`: The length of the path leading to this entry.
- `name`: The entry's name.
- `path`: The entry's path.
- `type`: The type of this file (`"File"` or `"Directory"`)
- `status`: How this path has changed since the base commit (`"New"`, `"Renamed"`, `"Removed"`, `"Modified"`, `"RenamedAndModified"`, or `"Unchanged"`)
- `visible`: Whether or not this file is currently visible (not deleted).

The `entries` method accepts two options as fields in an optional object passed to the method.

- `showDeleted`: If `true`, returns entries for deleted files and directories, but marks them as `visible: false`.
- `descendInto`: An optional array of paths. If provided, the traversal will skip descending into any directory not present in this whitelist. You can use this option to limit the number of entries you need to process if you are rendering a UI with collapsed directories.

## Creating, renaming and removing files

`WorkTree` APIs all function in terms of paths and allow you to manipulate files exactly as you would expect from a typical file system:

```ts
const op1 = tree.createFile("foo", memo.FileType.Directory);
const op2 = tree.createFile("foo/bar", memo.FileType.Text);
const op3 = tree.createFile("foo/baz", memo.FileType.Text);
const op4 = tree.rename("foo/bar", "foo/qux");
const op5 = tree.remove("foo/baz");
await broadcast([op1, op2, op3, op4, op5]);
```

## Buffers

To manipulate text files you'll need to call `openTextFile` with the path you want to open. This method will return a `Buffer` object that you can interact with:

```ts
const buffer = await tree.openTextFile("foo/qux");
const editOp1 = buffer.edit(
  [{ start: { row: 0, column: 0 }, end: { row: 0, column: 0 } }],
  "Hello, world!"
);
const editOp2 = buffer.edit(
  [{ start: { row: 0, column: 10 }, end: { row: 0, column: 12 } }],
  "ms"
);
await broadcast([editOp1, editOp2]);
console.log(buffer.getText()); // ==> "Hello worms"
```

As you incorporate operations received from other peers, you may want to use `Buffer.prototype.onChange` to keep an external representation of the buffer up-to-date:

```ts
buffer.onChange(changes => {
  for (const change of changes) {
    console.log(change); // => { start: { row: 0, column: 0 }, end: { row: 0, column: 5 }, text: "Goodbye" }
    externalBuffer.edit(change.start, change.end, change.text);
  }
});
```

## Resetting to a different base

If you want to reset the work tree to a different (possibly `null`) base (e.g. after a commit or a `git reset`), you can use the `reset` method:

```ts
const commitOid = await githubProvider.commit(tree.entries());
const resetOps = tree.reset(commitOid);
await broadcast(resetOps);
// tree now points to the new commit. Peers will be reset to the same commit
// upon receiving `resetOps`.
```

After switching to a new base all open buffers will still be valid and you can continue using them normally.

## Working with operations

All methods that update the state of the tree return _operations_, the fundamental primitive this library uses to synchronize with other peers. Sometimes operations are returned synchronously, sometimes they are async iterators instead. Make sure you handle both cases, as illustrated in the `broadcast` function later in this section.

In either case, operations are wrapped in an `OperationEnvelope`. An operation envelope is defined as follows:

```ts
export type OperationEnvelope = {
  epochTimestamp: number;
  epochReplicaId: string;
  operation: Operation;
};
```

Technically, to synchronize with other peers, you only need to transmit the Base64-encoded operation that is stored inside of the envelope; so, why including those extra timestamp and replica id fields?

You may recall the `fetchInitialOps` function that we called when [creating a new `WorkTree`](#creating-a-work-tree). It turns out that, in order to instantiate a new `WorkTree`, you only need operations associated with the _latest_ epoch. By exposing the epoch timestamp and replica id, we allow you to store operations such that they can be efficiently queried later when instantiating new work trees:

```ts
// Here we simulate having a database that stores every operation that has been
// generated.

async function fetchInitialOps(): Operation[] {
  // Note that this is very inefficient. In a production system, you should
  // perform the computation contained in this function on the database, using
  // an index on the (timestamp, replicaId) tuple.
  const allEnvelopes = await database.getAllOperationEnvelopes();

  // First we sort by timestamp, then by replica id.
  const sortedEnvelopes = database
    .getAllOperationEnvelopes()
    .sort(
      (a, b) =>
        a.epochTimestamp - b.epochTimestamp ||
        a.epochReplicaId - b.epochReplicaId
    );

  // Then, we only retrieve operations for the latest epoch.
  const lastEnvelope = sortedEnvelopes[sortedEnvelopes.length - 1];
  const latestEpochEnvelopes = sortedEnvelopes.filter(
    e =>
      e.epochTimestamp == lastEnvelope.epochTimestamp &&
      e.epochReplicaId == lastEnvelope.epochReplicaId
  );

  // Finally, we unwrap the envelopes and just return the operations inside.
  return latestEpochEnvelopes.map(envelope => envelope.operation);
}

async function broadcast(
  envelopes: OperationEnvelope[] | AsyncIterable<OperationEnvelope>
) {
  for await (const envelope of envelopes) {
    // Note how we store the full envelope in the database, but we only transmit
    // the operation inside of it to peers.
    database.store(envelope);
    network.broadcast(envelope.operation);
  }
}
```

So far we have covered storing and trasmitting operations sent by the local replica. To apply remote operations, you should use the `applyOps` method:

```ts
const remoteOps = await receiveOps();
const fixupOps = tree.applyOps(remoteOps);
await broadcast(fixupOps);
```

Whenever you call `applyOps`, there is a chance that additional "fixup" operations could be generated to deal with cycles and name conflicts in the tree. Be sure to broadcast these operations to peers to ensure convergence.
