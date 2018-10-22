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
    const [tree2, initOps2] = WorkTree.create(
      2,
      OID_0,
      await collect(initOps1),
      git
    );
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
    const c = await tree1.openTextFile("a/b/c");

    assert.strictEqual(tree1.getText(c), "abc");
    //   const editOperation = tree1.edit(
    //     buffer1,
    //     [
    //       { start: point(0, 0), end: point(0, 0) },
    //       { start: point(0, 1), end: point(0, 2) },
    //       { start: point(0, 3), end: point(0, 3) }
    //     ],
    //     "123"
    //   );
  });

  // test("basic API interaction", () => {
  //   const rootFileId = WorkTree.getRootFileId();
  //   const baseEntries = [
  //     { depth: 1, name: "a", type: memo.FileType.Directory },
  //     { depth: 2, name: "b", type: memo.FileType.Directory },
  //     { depth: 3, name: "c", type: memo.FileType.Text }
  //   ];

  //   const tree1 = new WorkTree(1);
  //   tree1.appendBaseEntries(baseEntries);
  //   const file1 = tree1.newTextFile();

  //   const tree2 = new WorkTree(2);
  //   tree2.appendBaseEntries(baseEntries);
  //   let file2 = tree2.newTextFile();

  //   assert.throws(() => {
  //     tree1.openTextFile(file2.fileId, "");
  //   });

  //   tree1.applyOps([file2.operation]);
  //   tree2.applyOps([file1.operation]);
  //   const buffer1 = tree1.openTextFile(file2.fileId, "abc");
  //   const editOperation = tree1.edit(
  //     buffer1,
  //     [
  //       { start: point(0, 0), end: point(0, 0) },
  //       { start: point(0, 1), end: point(0, 2) },
  //       { start: point(0, 3), end: point(0, 3) }
  //     ],
  //     "123"
  //   );

  //   const tree2VersionBeforeEdit = tree2.getVersion();
  //   tree2.applyOps([editOperation]);
  //   tree2.openTextFile(file2.fileId, "abc");
  //   assert.deepEqual(tree2.getText(buffer1), "123a123c123");
  //   assert.deepEqual(tree2.changesSince(buffer1, tree2VersionBeforeEdit), [
  //     { start: point(0, 0), end: point(0, 0), text: "123" },
  //     { start: point(0, 4), end: point(0, 5), text: "123" },
  //     { start: point(0, 8), end: point(0, 8), text: "123" }
  //   ]);

  //   const dir1 = tree1.createDirectory(rootFileId, "x");
  //   const dir2 = tree1.createDirectory(dir1.fileId, "y");
  //   assert.equal(tree1.pathForFileId(dir2.fileId), "x/y");
  //   assert.equal(tree1.fileIdForPath("x/y"), dir2.fileId);
  //   assert.equal(tree1.basePathForFileId(dir2.fileId), null);

  //   tree1.rename(dir1.fileId, tree1.fileIdForPath("a/b"), "x");
  //   assert.equal(tree1.fileIdForPath("a/b/x"), dir1.fileId);

  //   const c = tree1.fileIdForPath("a/b/c");
  //   tree1.remove(c);
  //   assert.equal(tree1.fileIdForPath("a/b/c"), null);
  //   assert.equal(tree1.pathForFileId(c), null);
  //   assert.equal(tree1.basePathForFileId(c), "a/b/c");

  //   assert.deepEqual(tree1.entries({ descendInto: [] }), [
  //     {
  //       depth: 1,
  //       fileId: tree1.fileIdForPath("a"),
  //       type: "Directory",
  //       name: "a",
  //       path: "a",
  //       status: "Unchanged",
  //       visible: true
  //     }
  //   ]);
  //   assert.deepEqual(
  //     tree1.entries({
  //       showDeleted: true,
  //       descendInto: [tree1.fileIdForPath("a"), tree1.fileIdForPath("a/b")]
  //     }),
  //     [
  //       {
  //         depth: 1,
  //         fileId: tree1.fileIdForPath("a"),
  //         type: "Directory",
  //         name: "a",
  //         path: "a",
  //         status: "Unchanged",
  //         visible: true
  //       },
  //       {
  //         depth: 2,
  //         fileId: tree1.fileIdForPath("a/b"),
  //         type: "Directory",
  //         name: "b",
  //         path: "a/b",
  //         status: "Unchanged",
  //         visible: true
  //       },
  //       {
  //         depth: 3,
  //         fileId: c,
  //         type: "Text",
  //         name: "c",
  //         path: "a/b/c",
  //         status: "Removed",
  //         visible: false
  //       },
  //       {
  //         depth: 3,
  //         fileId: dir1.fileId,
  //         type: "Directory",
  //         name: "x",
  //         path: "a/b/x",
  //         status: "New",
  //         visible: true
  //       }
  //     ]
  //   );
  // });
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

// function point(row: number, column: number): memo.Point {
//   return { row, column };
// }

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
