export let memoPromise = import("../dist/memo_wasm");

async function initialize() {
  const memo = await memoPromise;

  class WorkTree {
    constructor(replicaId) {
      this.workTree = memo.WorkTree.new(BigInt(replicaId));
    }

    appendBaseEntries(baseEntries) {
      for (const baseEntry of baseEntries) {
        this.workTree.push_base_entry(
          baseEntry.depth,
          baseEntry.name,
          baseEntry.type
        );
      }
      return collect(this.workTree.flush_base_entries());
    }

    applyOps(ops) {
      for (const op of ops) {
        this.workTree.push_op(op);
      }
      return collect(this.workTree.flush_ops());
    }

    newTextFile() {
      let result = this.workTree.new_text_file();
      return { fileId: result.file_id(), operation: result.operation() };
    }

    newDirectory(parentId, name) {
      let result = this.workTree.new_directory(parentId, name);
      return { fileId: result.file_id(), operation: result.operation() };
    }
  }

  return { WorkTree, FileType: memo.FileType };
}

function collect(iterator) {
  let items = [];
  while (iterator.has_next()) {
    items.push(iterator.next());
  }
  return items;
}

async function test() {
  const { WorkTree, FileType } = await initialize();

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

  tree1.applyOps([file2.operation]);
  tree2.applyOps([file1.operation]);
}

test();
