export let memoPromise = import("../dist/memo_wasm");

async function load() {
  let {WorkTree, FileType} = await memoPromise;
  let tree1 = WorkTree.new(BigInt(1));
  tree1.append_base_entry(1, "asd", FileType.Directory);
  tree1.append_base_entry(2, "foo", FileType.Directory);
  tree1.append_base_entry(3, "bar", FileType.Text);
  tree1.flush_base_entries();

  let tree2 = WorkTree.new(BigInt(1));
  tree2.append_base_entry(1, "asd", FileType.Directory);
  tree2.append_base_entry(2, "foo", FileType.Directory);
  tree2.append_base_entry(3, "bar", FileType.Text);
  let {file_id, operation} = tree2.new_text_file();

  
}

load();
