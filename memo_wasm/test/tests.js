const memo = require("../dist");
const assert = require("assert");

suite("WorkTree", () => {
  let WorkTree, FileType;

  suiteSetup(async () => {
    ({WorkTree, FileType} = await memo.initialize());
  });

  test("basic API interaction", () => {
    const baseEntries = [
      { depth: 1, name: "a", type: FileType.Directory },
      { depth: 2, name: "b", type: FileType.Directory },
      { depth: 3, name: "c", type: FileType.Text }
    ];

    const tree1 = new WorkTree(1);
    tree1.appendBaseEntries(baseEntries);
    let file1 = tree1.newTextFile();

    const tree2 = new WorkTree(2);
    tree2.appendBaseEntries(baseEntries);
    let file2 = tree2.newTextFile();

    assert.throws(() => {
      tree1.openTextFile(file2.fileId, "");
    })

    tree1.applyOps([file2.operation]);
    tree2.applyOps([file1.operation]);
    const buffer1 = tree1.openTextFile(file2.fileId, "");
  });
});
