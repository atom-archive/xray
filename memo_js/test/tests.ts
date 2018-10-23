import * as memo from "../src/index";
import * as assert from "assert";

suite("WorkTree", () => {
  let WorkTree: typeof memo.WorkTree;

  suiteSetup(async () => {
    ({ WorkTree } = await memo.init());
  });

  test("basic API interaction", async () => {
    const OID_0 = "0".repeat(40);

    const git = new TestGitProvider();
    git.commit(OID_0, [
      { depth: 1, name: "a", type: memo.FileType.Directory },
      { depth: 2, name: "b", type: memo.FileType.Directory },
      { depth: 3, name: "c", type: memo.FileType.Text, text: "abc" }
    ]);

    const [tree1, initOps1] = WorkTree.create(1, OID_0, [], git);
    const [tree2, initOps2] = WorkTree.create(2, OID_0, await collect(initOps1), git);
    assert.strictEqual((await collect(initOps2)).length, 0);

    const ops1 = [];
    const ops2 = [];
    ops1.push(tree1.createFile("d", memo.FileType.Text));
    ops2.push(tree2.createFile("e", memo.FileType.Text));

    await assert.rejects(() => tree2.openTextFile("d"));

    ops1.push(...(await collect(tree1.applyOps(ops2.splice(0, Infinity)))));
    ops2.push(...(await collect(tree2.applyOps(ops1.splice(0, Infinity)))));
    assert.strictEqual(ops1.length, 0);
    assert.strictEqual(ops2.length, 0);

    const d = await tree1.openTextFile("d");
    const tree1BufferC = await tree1.openTextFile("a/b/c");

    assert.strictEqual(tree1.getText(tree1BufferC), "abc");
    ops1.push(
      tree1.edit(
        tree1BufferC,
        [
          { start: point(0, 0), end: point(0, 0) },
          { start: point(0, 1), end: point(0, 2) },
          { start: point(0, 3), end: point(0, 3) }
        ],
        "123"
      )
    );
    assert.strictEqual(tree1.getText(tree1BufferC), "123a123c123");

    const tree2VersionBeforeEdit = tree2.getVersion();
    assert.deepStrictEqual(await collect(tree2.applyOps(ops1)), []);
    ops1.length = 0;
    const tree2BufferC = await tree2.openTextFile("a/b/c");
    assert.strictEqual(tree2.getText(tree2BufferC), "123a123c123");
    assert.deepEqual(tree2.changesSince(tree2BufferC, tree2VersionBeforeEdit), [
      { start: point(0, 0), end: point(0, 0), text: "123" },
      { start: point(0, 4), end: point(0, 5), text: "123" },
      { start: point(0, 8), end: point(0, 8), text: "123" }
    ]);

    ops1.push(tree1.createFile("x", memo.FileType.Directory));
    ops1.push(tree1.createFile("x/y", memo.FileType.Directory));
    ops1.push(tree1.rename("x", "a/b/x"));
    ops1.push(tree1.remove("a/b/c"));
    assert.deepStrictEqual(await collect(tree2.applyOps(ops1)), []);
    assert.deepStrictEqual(await collect(tree1.applyOps(ops2)), []);
    ops1.length = 0;
    ops2.length = 0;

    assert.deepStrictEqual(tree1.entries(), tree2.entries());
    assert.deepEqual(tree1.entries({ descendInto: [] }), [
      {
        depth: 1,
        type: memo.FileType.Directory,
        name: "a",
        path: "a",
        status: memo.FileStatus.Unchanged,
        visible: true
      },
      {
        depth: 1,
        type: memo.FileType.Text,
        name: "d",
        path: "d",
        status: memo.FileStatus.New,
        visible: true
      },
      {
        depth: 1,
        type: memo.FileType.Text,
        name: "e",
        path: "e",
        status: memo.FileStatus.New,
        visible: true
      },
    ]);
    assert.deepEqual(tree1.entries({ showDeleted: true, descendInto: ["a", "a/b"] }), [
      {
        depth: 1,
        type: memo.FileType.Directory,
        name: "a",
        path: "a",
        status: memo.FileStatus.Unchanged,
        visible: true
      },
      {
        depth: 2,
        type: memo.FileType.Directory,
        name: "b",
        path: "a/b",
        status: memo.FileStatus.Unchanged,
        visible: true
      },
      {
        depth: 3,
        type: memo.FileType.Text,
        name: "c",
        path: "a/b/c",
        status: memo.FileStatus.Removed,
        visible: false
      },
      {
        depth: 3,
        type: memo.FileType.Directory,
        name: "x",
        path: "a/b/x",
        status: memo.FileStatus.New,
        visible: true
      },
      {
        depth: 1,
        type: memo.FileType.Text,
        name: "d",
        path: "d",
        status: memo.FileStatus.New,
        visible: true
      },
      {
        depth: 1,
        type: memo.FileType.Text,
        name: "e",
        path: "e",
        status: memo.FileStatus.New,
        visible: true
      },
    ]);
  });
});

type BaseEntry =
  | memo.BaseEntry & { type: memo.FileType.Directory }
  | memo.BaseEntry & { type: memo.FileType.Text; text: string };

async function collect<T>(iterable: AsyncIterable<T>): Promise<T[]> {
  const items = [];
  for await (const item of iterable) {
    items.push(item);
  }
  return items;
}

function point(row: number, column: number): memo.Point {
  return { row, column };
}

class TestGitProvider implements memo.GitProvider {
  private entries: Map<memo.Oid, ReadonlyArray<memo.BaseEntry>>;
  private text: Map<memo.Oid, Map<memo.Path, string>>;

  constructor() {
    this.entries = new Map();
    this.text = new Map();
  }

  commit(oid: memo.Oid, entries: ReadonlyArray<BaseEntry>) {
    this.entries.set(oid, entries);

    const textByPath = new Map();
    const path = [];
    for (const entry of entries) {
      path.length = entry.depth - 1;
      path.push(entry.name);
      if (entry.type === memo.FileType.Text) {
        textByPath.set(path.join("/"), entry.text);
      }
    }
    this.text.set(oid, textByPath);
  }

  async *baseEntries(oid: memo.Oid): AsyncIterable<memo.BaseEntry> {
    const entries = this.entries.get(oid);
    if (entries) {
      for (const entry of entries) {
        yield entry;
      }
    } else {
      throw new Error("yy");
    }
  }

  async baseText(oid: memo.Oid, path: memo.Path): Promise<string> {
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
