const memo = require("../dist");
const assert = require("assert");

suite("WorkTree", () => {
  let WorkTree;

  suiteSetup(async () => {
    ({ WorkTree } = await memo.initialize());
  });

  test("basic API interaction", () => {
    const rootFileId = WorkTree.getRootFileId();
    const baseEntries = [
      { depth: 1, name: "a", file_type: "Directory" },
      { depth: 2, name: "b", file_type: "Directory" },
      { depth: 3, name: "c", file_type: "Text" }
    ];

    const tree1 = new WorkTree(1);
    tree1.appendBaseEntries(baseEntries);
    const file1 = tree1.newTextFile();
    const dir1 = tree1.createDirectory(rootFileId, "x");

    const tree2 = new WorkTree(2);
    tree2.appendBaseEntries(baseEntries);
    let file2 = tree2.newTextFile();

    assert.throws(() => {
      tree1.openTextFile(file2.fileId, "");
    });

    tree1.applyOps([file2.operation]);
    tree2.applyOps([file1.operation]);
    const buffer1 = tree1.openTextFile(file2.fileId, "abc");
    const editOperation = tree1.edit(
      buffer1,
      [{ start: 0, end: 0 }, { start: 1, end: 2 }, { start: 3, end: 3 }],
      "123"
    );

    const tree2VersionBeforeEdit = tree2.getVersion();
    tree2.applyOps([editOperation]);
    tree2.openTextFile(file2.fileId, "abc");
    assert.deepEqual(tree2.changesSince(buffer1, tree2VersionBeforeEdit), [
      { start: 0, end: 0, text: "123" },
      { start: 4, end: 5, text: "123" },
      { start: 8, end: 8, text: "123" }
    ]);
  });
});
