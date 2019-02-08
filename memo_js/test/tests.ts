import {
  BaseEntry as MemoBaseEntry,
  Change,
  GitProvider,
  FileStatus,
  FileType,
  Oid,
  Operation,
  OperationEnvelope,
  Path,
  Point,
  ReplicaId,
  SelectionRanges,
  WorkTree
} from "../src/index";
import * as assert from "assert";
import * as uuid from "uuid/v4";
import * as uuidParse from "uuid-parse";

suite("WorkTree", () => {
  test("basic API interaction", async () => {
    const OID_0 = "0".repeat(40);
    const OID_1 = "1".repeat(40);

    const git = new TestGitProvider();
    git.commit(OID_0, [
      { depth: 1, name: "a", type: FileType.Directory },
      { depth: 2, name: "b", type: FileType.Directory },
      { depth: 3, name: "c", type: FileType.Text, text: "oid0 base text" },
      { depth: 3, name: "d", type: FileType.Directory }
    ]);
    git.commit(OID_1, [
      { depth: 1, name: "a", type: FileType.Directory },
      { depth: 2, name: "b", type: FileType.Directory },
      { depth: 3, name: "c", type: FileType.Text, text: "oid1 base text" }
    ]);

    const [tree1, initOps1] = await WorkTree.create(uuid(), OID_0, [], git);
    const [tree2, initOps2] = await WorkTree.create(
      uuid(),
      OID_0,
      await collectOps(initOps1),
      git
    );
    assert.strictEqual((await collectOps(initOps2)).length, 0);
    assert.strictEqual(tree1.head(), OID_0);
    assert.strictEqual(tree2.head(), OID_0);

    const ops1 = [];
    const ops2 = [];
    ops1.push(tree1.createFile("e", FileType.Text).operation());
    ops2.push(tree2.createFile("f", FileType.Text).operation());

    await assert.rejects(() => tree2.openTextFile("e"));

    ops1.push(...(await collectOps(tree1.applyOps(ops2.splice(0, Infinity)))));
    ops2.push(...(await collectOps(tree2.applyOps(ops1.splice(0, Infinity)))));
    assert.strictEqual(ops1.length, 0);
    assert.strictEqual(ops2.length, 0);

    const tree1BufferC = await tree1.openTextFile("a/b/c");
    assert.strictEqual(tree1BufferC.getPath(), "a/b/c");
    assert.strictEqual(tree1BufferC.getText(), "oid0 base text");
    assert.strictEqual(tree1BufferC.getDeferredOperationCount(), 0);
    const tree2BufferC = await tree2.openTextFile("a/b/c");
    assert.strictEqual(tree2BufferC.getPath(), "a/b/c");
    assert.strictEqual(tree2BufferC.getText(), "oid0 base text");
    assert.strictEqual(tree1BufferC.getDeferredOperationCount(), 0);

    const tree1BufferChanges: Change[] = [];
    tree1BufferC.onChange(c => tree1BufferChanges.push(...c.textChanges));
    ops1.push(
      tree1BufferC
        .edit(
          [
            { start: point(0, 4), end: point(0, 5) },
            { start: point(0, 9), end: point(0, 10) }
          ],
          "-"
        )
        .operation()
    );
    assert.strictEqual(tree1BufferC.getText(), "oid0-base-text");

    const tree2BufferChanges: Change[] = [];
    tree2BufferC.onChange(c => tree2BufferChanges.push(...c.textChanges));
    assert.deepStrictEqual(await collectOps(tree2.applyOps(ops1)), []);
    assert.strictEqual(tree1BufferC.getText(), "oid0-base-text");
    assert.deepStrictEqual(tree1BufferChanges, []);
    assert.deepStrictEqual(tree2BufferChanges, [
      { start: point(0, 4), end: point(0, 5), text: "-" },
      { start: point(0, 9), end: point(0, 10), text: "-" }
    ]);
    ops1.length = 0;

    ops1.push(tree1.createFile("x", FileType.Directory).operation());
    ops1.push(tree1.createFile("x/y", FileType.Directory).operation());
    ops1.push(tree1.rename("x", "a/b/x").operation());
    ops1.push(tree1.remove("a/b/d").operation());
    assert.deepStrictEqual(await collectOps(tree2.applyOps(ops1)), []);
    assert.deepStrictEqual(await collectOps(tree1.applyOps(ops2)), []);
    ops1.length = 0;
    ops2.length = 0;

    assert.deepStrictEqual(tree1.entries(), tree2.entries());
    assert.deepEqual(tree1.entries({ descendInto: [] }), [
      {
        depth: 1,
        type: FileType.Directory,
        name: "a",
        path: "a",
        status: FileStatus.Unchanged,
        visible: true
      },
      {
        depth: 1,
        type: FileType.Text,
        name: "e",
        path: "e",
        status: FileStatus.New,
        visible: true
      },
      {
        depth: 1,
        type: FileType.Text,
        name: "f",
        path: "f",
        status: FileStatus.New,
        visible: true
      }
    ]);
    assert.deepEqual(
      tree1.entries({ showDeleted: true, descendInto: ["a", "a/b"] }),
      [
        {
          depth: 1,
          type: FileType.Directory,
          name: "a",
          path: "a",
          status: FileStatus.Unchanged,
          visible: true
        },
        {
          depth: 2,
          type: FileType.Directory,
          name: "b",
          path: "a/b",
          status: FileStatus.Unchanged,
          visible: true
        },
        {
          depth: 3,
          type: FileType.Text,
          name: "c",
          path: "a/b/c",
          status: FileStatus.Modified,
          visible: true
        },
        {
          depth: 3,
          type: FileType.Directory,
          name: "d",
          path: "a/b/d",
          status: FileStatus.Removed,
          visible: false
        },
        {
          depth: 3,
          type: FileType.Directory,
          name: "x",
          path: "a/b/x",
          status: FileStatus.New,
          visible: true
        },
        {
          depth: 1,
          type: FileType.Text,
          name: "e",
          path: "e",
          status: FileStatus.New,
          visible: true
        },
        {
          depth: 1,
          type: FileType.Text,
          name: "f",
          path: "f",
          status: FileStatus.New,
          visible: true
        }
      ]
    );
    assert(tree1.exists("a/b/x"));
    assert(!tree1.exists("a/b/d"));

    tree1BufferChanges.length = 0;
    tree2BufferChanges.length = 0;
    ops1.push(...(await collectOps(tree1.reset(OID_1))));
    assert.deepStrictEqual(await collect(tree2.applyOps(ops1)), []);
    assert.strictEqual(tree1.head(), OID_1);
    assert.strictEqual(tree2.head(), OID_1);
    assert.strictEqual(tree1BufferC.getText(), "oid1 base text");
    assert.strictEqual(tree2BufferC.getText(), "oid1 base text");
    assert.deepStrictEqual(tree1BufferChanges, [
      { start: point(0, 3), end: point(0, 5), text: "1 " },
      { start: point(0, 9), end: point(0, 10), text: " " }
    ]);
    assert.deepStrictEqual(tree2BufferChanges, [
      { start: point(0, 3), end: point(0, 5), text: "1 " },
      { start: point(0, 9), end: point(0, 10), text: " " }
    ]);

    tree1.remove("a/b/c");
    assert(tree1BufferC.getPath() == null);

    await collectOps(tree1.reset(null));
    assert.strictEqual(tree1.head(), null);
  });

  test("selections", async () => {
    const OID = "0".repeat(40);
    const git = new TestGitProvider();
    git.commit(OID, [
      { depth: 1, name: "a", type: FileType.Text, text: "abc" }
    ]);

    const replica1 = uuid();
    const replica2 = uuid();
    const [tree1, initOps1] = await WorkTree.create(replica1, OID, [], git);
    const [tree2, initOps2] = await WorkTree.create(
      replica2,
      OID,
      await collectOps(initOps1),
      git
    );
    await collectOps(initOps2);

    const buffer1 = await tree1.openTextFile("a");
    let selection2Changes: Array<SelectionRanges> = [];
    const buffer2 = await tree2.openTextFile("a");
    buffer2.onChange(c => selection2Changes.push(c.selectionRanges));

    const [set, createSetOp] = buffer1.addSelectionSet([
      { start: point(0, 0), end: point(0, 1) }
    ]);
    assert.deepEqual(mapToObject(buffer1.getSelectionRanges().local), {
      [set]: [{ start: point(0, 0), end: point(0, 1) }]
    });
    assert.deepEqual(mapToObject(buffer1.getSelectionRanges().remote), {});

    await collectOps(tree2.applyOps([createSetOp.operation()]));

    assert.deepEqual(mapToObject(buffer2.getSelectionRanges().local), {});
    assert.deepEqual(mapToObject(buffer2.getSelectionRanges().remote), {
      [replica1]: [[{ start: point(0, 0), end: point(0, 1) }]]
    });

    assert.equal(selection2Changes.length, 1);
    assert.deepEqual(last(selection2Changes), buffer2.getSelectionRanges());

    const replaceSetOp = buffer1.replaceSelectionSet(set, [
      { start: point(0, 2), end: point(0, 3) }
    ]);
    assert.deepEqual(mapToObject(buffer1.getSelectionRanges().local), {
      [set]: [{ start: point(0, 2), end: point(0, 3) }]
    });
    assert.deepEqual(mapToObject(buffer1.getSelectionRanges().remote), {});

    await collectOps(tree2.applyOps([replaceSetOp.operation()]));

    assert.deepEqual(mapToObject(buffer2.getSelectionRanges().local), {});
    assert.deepEqual(mapToObject(buffer2.getSelectionRanges().remote), {
      [replica1]: [[{ start: point(0, 2), end: point(0, 3) }]]
    });

    assert.equal(selection2Changes.length, 2);
    assert.deepEqual(last(selection2Changes), buffer2.getSelectionRanges());

    const removeSetOp = buffer1.removeSelectionSet(set);
    assert.deepEqual(mapToObject(buffer1.getSelectionRanges().local), {});
    assert.deepEqual(mapToObject(buffer1.getSelectionRanges().remote), {});

    await collectOps(tree2.applyOps([removeSetOp.operation()]));

    assert.deepEqual(mapToObject(buffer2.getSelectionRanges().local), {});
    assert.deepEqual(mapToObject(buffer2.getSelectionRanges().remote), {});

    assert.equal(selection2Changes.length, 3);
    assert.deepEqual(last(selection2Changes), buffer2.getSelectionRanges());
  });

  test("active location", async () => {
    const oid = "0".repeat(40);
    const git = new TestGitProvider();
    git.commit(oid, [
      { depth: 1, name: "a", type: FileType.Text, text: "a" },
      { depth: 1, name: "b", type: FileType.Text, text: "b" }
    ]);

    const replicaId1 = uuid();
    const [tree1, initOps1] = await WorkTree.create(replicaId1, oid, [], git);

    const replicaId2 = uuid();
    const [tree2, initOps2] = await WorkTree.create(
      replicaId2,
      oid,
      await collectOps(initOps1),
      git
    );
    assert.strictEqual((await collectOps(initOps2)).length, 0);

    const buffer1 = await tree1.openTextFile("a");
    const buffer2 = await tree2.openTextFile("b");

    await collectOps(
      tree1.applyOps([tree2.setActiveLocation(buffer2).operation()])
    );
    await collectOps(
      tree2.applyOps([tree1.setActiveLocation(buffer1).operation()])
    );
    assert.deepStrictEqual(mapToObject(tree1.getReplicaLocations()), {
      [replicaId1]: "a",
      [replicaId2]: "b"
    });
    assert.deepStrictEqual(mapToObject(tree2.getReplicaLocations()), {
      [replicaId1]: "a",
      [replicaId2]: "b"
    });

    await collectOps(
      tree1.applyOps([tree2.setActiveLocation(null).operation()])
    );
    assert.deepStrictEqual(mapToObject(tree1.getReplicaLocations()), {
      [replicaId1]: "a"
    });
    assert.deepStrictEqual(mapToObject(tree2.getReplicaLocations()), {
      [replicaId1]: "a"
    });
  });

  test("an invalid base commit oid throws an error instead of crashing", async () => {
    assert.rejects(
      () => WorkTree.create(uuid(), "12345678", [], new TestGitProvider()),
      /12345678/
    );
  });

  test("opening a directory as a text file", async () => {
    const git = new TestGitProvider();
    const [tree] = await WorkTree.create(uuid(), null, [], git);
    tree.createFile("dir", FileType.Directory);
    assert.rejects(tree.openTextFile("dir"), /text/i);
  });

  test("the epoch head is available on operation envelopes", async () => {
    const OID = "0".repeat(40);

    const git = new TestGitProvider();
    git.commit(OID, [{ depth: 1, name: "a", type: FileType.Directory }]);

    const [tree1] = await WorkTree.create(uuid(), null, [], git);
    const envelope1 = tree1.createFile("x", FileType.Text);
    assert.strictEqual(envelope1.epochHead(), null);
    const [envelope2] = await collect(tree1.reset(OID));
    assert.strictEqual(envelope2.epochHead(), OID);
    const envelope3 = tree1.createFile("y", FileType.Text);
    assert.strictEqual(envelope3.epochHead(), OID);
  });

  test("epoch id", async () => {
    const git = new TestGitProvider();
    const replicaId = uuid();
    const [tree] = await WorkTree.create(replicaId, null, [], git);
    const envelope1 = tree.createFile("a", FileType.Text);
    const envelope1EpochId = parseEpochId(envelope1.epochId());
    const envelope2 = tree.createFile("b", FileType.Text);
    const envelope2EpochId = parseEpochId(envelope2.epochId());
    assert.deepStrictEqual(envelope1EpochId, envelope2EpochId);
    assert.equal(envelope1EpochId.replicaId, replicaId);
  });

  test("replica id", async () => {
    const git = new TestGitProvider();

    {
      const replicaId = uuid();
      const [tree] = await WorkTree.create(replicaId, null, [], git);
      const envelope = tree.createFile("x", FileType.Text);
      assert.strictEqual(envelope.epochReplicaId(), replicaId);
    }

    {
      await assert.rejects(
        WorkTree.create("invalid-replica-id", null, [], git),
        /invalid-replica-id/
      );
    }
  });

  test("isSelectionUpdate", async () => {
    const git = new TestGitProvider();
    const [tree] = await WorkTree.create(uuid(), null, [], git);

    const envelope1 = tree.createFile("file", FileType.Text);
    assert(!envelope1.isSelectionUpdate());

    const buffer1 = await tree.openTextFile("file");
    const envelope2 = buffer1.edit(
      [{ start: point(0, 0), end: point(0, 0) }],
      "abc"
    );
    assert(!envelope2.isSelectionUpdate());

    const [, envelope3] = buffer1.addSelectionSet([]);
    assert(envelope3.isSelectionUpdate());
  });

  test("versions", async () => {
    const OID = "0".repeat(40);

    const git = new TestGitProvider();
    git.commit(OID, [{ depth: 1, name: "a", type: FileType.Directory }]);

    const [tree1, initOps1] = await WorkTree.create(uuid(), OID, [], git);
    const [tree2, initOps2] = await WorkTree.create(
      uuid(),
      OID,
      await collectOps(initOps1),
      git
    );
    assert.deepEqual(await collectOps(initOps2), []);
    assert(tree1.hasObserved(tree2.version()));
    assert(tree2.hasObserved(tree1.version()));

    let op1 = tree1.createFile("a/b.txt", FileType.Text);
    let op2 = tree2.createFile("a/c.txt", FileType.Text);
    assert(!tree1.hasObserved(tree2.version()));
    assert(!tree2.hasObserved(tree1.version()));

    await collectOps(tree1.applyOps([op2.operation()]));
    assert(tree1.hasObserved(tree2.version()));
    await collectOps(tree2.applyOps([op1.operation()]));
    assert(tree2.hasObserved(tree1.version()));
  });

  test("buffer reuse", async () => {
    const git = new TestGitProvider();
    const replicaId = uuid();
    const [tree] = await WorkTree.create(replicaId, null, [], git);
    tree.createFile("a", FileType.Text);
    assert.equal(await tree.openTextFile("a"), await tree.openTextFile("a"));
  });

  test("buffer.onChange disposal", async () => {
    const OID = "0".repeat(40);
    const git = new TestGitProvider();
    git.commit(OID, [
      { depth: 1, name: "a", type: FileType.Directory },
      { depth: 2, name: "b", type: FileType.Directory },
      { depth: 3, name: "c", type: FileType.Text, text: "oid0 base text" },
      { depth: 3, name: "d", type: FileType.Directory }
    ]);

    const [tree1, initOps1] = await WorkTree.create(uuid(), OID, [], git);
    const [tree2, initOps2] = await WorkTree.create(
      uuid(),
      OID,
      await collectOps(initOps1),
      git
    );
    tree1.applyOps(await collectOps(initOps2));

    const buffer1 = await tree1.openTextFile("a/b/c");
    let buffer1ChangeCount = 0;
    const disposable = buffer1.onChange(_ => buffer1ChangeCount++);

    const buffer2 = await tree2.openTextFile("a/b/c");
    tree1.applyOps([
      buffer2.edit([{ start: point(0, 0), end: point(0, 0) }], "x").operation()
    ]);
    assert.strictEqual(buffer1ChangeCount, 1);

    disposable.dispose();
    tree1.applyOps([
      buffer2.edit([{ start: point(0, 0), end: point(0, 0) }], "y").operation()
    ]);
    assert.strictEqual(buffer1ChangeCount, 1);
  });

  test("throwing error when retrieving base entries", async () => {
    const git = {
      async *baseEntries(): AsyncIterable<BaseEntry> {
        await 0;
        throw new Error("baseEntries error");
      },
      async baseText(): Promise<string> {
        await 0;
        throw new Error("baseText");
      }
    };
    const OID = "0".repeat(40);
    const [, initOps] = await WorkTree.create(uuid(), OID, [], git);
    assert.rejects(collectOps(initOps), /baseEntries error/);
  });

  test("throwing error when retrieving base text", async () => {
    const git = {
      async *baseEntries(): AsyncIterable<BaseEntry> {
        await 0;
        yield { depth: 1, name: "a", type: FileType.Text, text: "base text" };
      },
      async baseText(): Promise<string> {
        await 0;
        throw new Error("baseText");
      }
    };
    const OID = "0".repeat(40);
    const [tree, initOps] = await WorkTree.create(uuid(), OID, [], git);
    await collectOps(initOps);
    assert.rejects(tree.openTextFile("a"), /baseText/);
  });
});

type BaseEntry =
  | MemoBaseEntry & { type: FileType.Directory }
  | MemoBaseEntry & { type: FileType.Text; text: string };

async function collect<T>(iterable: AsyncIterable<T>): Promise<T[]> {
  const items = [];
  for await (const item of iterable) {
    items.push(item);
  }
  return items;
}

async function collectOps(
  ops: AsyncIterable<OperationEnvelope>
): Promise<Operation[]> {
  const envelopes = await collect(ops);
  return envelopes.map(envelope => envelope.operation());
}

function point(row: number, column: number): Point {
  return { row, column };
}

type ParsedEpochId = { timestamp: number; replicaId: ReplicaId };

function parseEpochId(epochId: Uint8Array): ParsedEpochId {
  const epochIdBuffer = Buffer.from(epochId);
  assert.equal(epochIdBuffer.length, 24);
  // Timestamp is a u64 but JavaScript doesn't support it, so we fail if its
  // high bits are not 0.
  assert.equal(epochIdBuffer.readUInt32BE(0), 0);
  const timestamp = epochIdBuffer.readUInt32BE(1);
  const replicaId = uuidParse.unparse(epochIdBuffer.slice(8)) as ReplicaId;
  return { timestamp, replicaId };
}

function mapToObject<T>(map: Map<string | number, T>): { [key: string]: T } {
  const object: { [key: string]: T } = {};
  map.forEach((value, key) => {
    object[key] = value;
  });
  return object;
}

function last<T>(array: ArrayLike<T>): undefined | T {
  return array[array.length - 1];
}

class TestGitProvider implements GitProvider {
  private entries: Map<Oid, ReadonlyArray<BaseEntry>>;
  private text: Map<Oid, Map<Path, string>>;

  constructor() {
    this.entries = new Map();
    this.text = new Map();
  }

  commit(oid: Oid, entries: ReadonlyArray<BaseEntry>) {
    this.entries.set(oid, entries);

    const textByPath = new Map();
    const path = [];
    for (const entry of entries) {
      path.length = entry.depth - 1;
      path.push(entry.name);
      if (entry.type === FileType.Text) {
        textByPath.set(path.join("/"), entry.text);
      }
    }
    this.text.set(oid, textByPath);
  }

  async *baseEntries(oid: Oid): AsyncIterable<BaseEntry> {
    const entries = this.entries.get(oid);
    if (entries) {
      for (const entry of entries) {
        yield entry;
      }
    } else {
      throw new Error("yy");
    }
  }

  async baseText(oid: Oid, path: Path): Promise<string> {
    const textByPath = this.text.get(oid);
    if (textByPath != null) {
      const text = textByPath.get(path);
      if (text != null) {
        await Promise.resolve();
        return text;
      } else {
        throw new Error(`No text found at path ${path}`);
      }
    } else {
      throw new Error(`No commit found with oid ${oid}`);
    }
  }
}
