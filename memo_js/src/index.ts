export { BaseEntry, GitProvider, FileType, Oid, Path } from "./support";
import {
  GitProvider,
  GitProviderWrapper,
  FileType,
  Oid,
  Path
} from "./support";

let memo: any;

export async function init() {
  memo = await import("../dist/memo_js");
  memo.StreamToAsyncIterator.prototype[Symbol.asyncIterator] = function() {
    return this;
  };
  return { WorkTree };
}

type Tagged<BaseType, TagName> = BaseType & { __tag: TagName };

export type BufferId = Tagged<number, "BufferId">;
export type Version = Tagged<object, "Version">;
export type Operation = Tagged<string, "Operation">;
export type Point = { row: number; column: number };
export type Range = { start: Point; end: Point };

export class WorkTree {
  private tree: any;

  static create(
    replicaId: number,
    base: Oid,
    startOps: ReadonlyArray<Operation>,
    git: GitProvider
  ): [WorkTree, AsyncIterable<Operation>] {
    const result = memo.WorkTree.new(new GitProviderWrapper(git), {
      replica_id: replicaId,
      base,
      start_ops: startOps
    });
    return [new WorkTree(result.tree()), result.operations()];
  }

  constructor(tree: any) {
    this.tree = tree;
  }

  applyOps(ops: Operation[]): AsyncIterable<Operation> {
    return this.tree.apply_ops(ops);
  }

  createFile(path: Path, fileType: FileType): Operation {
    return this.tree.create_file({
      path,
      file_type: fileType
    });
  }

  openTextFile(path: Path): Promise<BufferId> {
    return this.tree.open_text_file(path);
  }

  getText(bufferId: BufferId): string {
    return this.tree.text(bufferId);
  }

  edit(bufferId: BufferId, oldRanges: Range[], newText: string): Operation {
    return this.tree.edit(bufferId, oldRanges, newText);
  }
}
