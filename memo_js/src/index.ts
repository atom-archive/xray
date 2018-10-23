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
export type Version = Tagged<string, "Version">;
export type Operation = Tagged<string, "Operation">;
export type Point = { row: number; column: number };
export type Range = { start: Point; end: Point };
export type Change = Range & { text: string };
export enum FileStatus {
  New = "New",
  Renamed = "Renamed",
  Removed = "Removed",
  Modified = "Modified",
  RenamedAndModified = "RenamedAndModified",
  Unchanged = "Unchanged"
}

export interface Entry {
  readonly depth: number;
  readonly type: FileType;
  readonly name: string;
  readonly path: string;
  readonly status: FileStatus;
  readonly visible: boolean;
}

export class WorkTree {
  private tree: any;

  static create(
    replicaId: number,
    base: Oid,
    startOps: ReadonlyArray<Operation>,
    git: GitProvider
  ): [WorkTree, AsyncIterable<Operation>] {
    const result = memo.WorkTree.new(
      new GitProviderWrapper(git),
      replicaId,
      base,
      startOps
    );
    return [new WorkTree(result.tree()), result.operations()];
  }

  constructor(tree: any) {
    this.tree = tree;
  }

  getVersion(): Version {
    return this.tree.version();
  }

  applyOps(ops: Operation[]): AsyncIterable<Operation> {
    return this.tree.apply_ops(ops);
  }

  createFile(path: Path, fileType: FileType): Operation {
    return this.tree.create_file(path, fileType);
  }

  rename(oldPath: Path, newPath: Path): Operation {
    return this.tree.rename(oldPath, newPath);
  }

  remove(path: Path): Operation {
    return this.tree.remove(path);
  }

  entries(options?: { descendInto?: Path[]; showDeleted?: boolean }): Entry[] {
    let descendInto = null;
    let showDeleted = false;
    if (options) {
      if (options.descendInto) descendInto = options.descendInto;
      if (options.showDeleted) showDeleted = options.showDeleted;
    }
    return this.tree.entries(descendInto, showDeleted);
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

  changesSince(bufferId: BufferId, version: Version): Change[] {
    return this.tree.changes_since(bufferId, version);
  }
}
