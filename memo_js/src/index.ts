export {
  BaseEntry,
  Change,
  GitProvider,
  FileType,
  Oid,
  Path,
  Point,
  Range
} from "./support";
import {
  BufferId,
  Change,
  ChangeObserver,
  ChangeObserverCallback,
  GitProvider,
  GitProviderWrapper,
  FileType,
  Oid,
  Path,
  Point,
  Range,
  Tagged
} from "./support";

let memo: any;

export async function init() {
  memo = await import("../dist/memo_js");
  memo.StreamToAsyncIterator.prototype[Symbol.asyncIterator] = function() {
    return this;
  };
  return { WorkTree };
}

export type Version = Tagged<string, "Version">;
export type Operation = Tagged<string, "Operation">;

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
  private observer: ChangeObserver;

  static create(
    replicaId: number,
    base: Oid,
    startOps: ReadonlyArray<Operation>,
    git: GitProvider
  ): [WorkTree, AsyncIterable<Operation>] {
    const observer = new ChangeObserver();
    const result = memo.WorkTree.new(
      new GitProviderWrapper(git),
      observer,
      replicaId,
      base,
      startOps
    );
    return [new WorkTree(result.tree(), observer), result.operations()];
  }

  private constructor(tree: any, observer: ChangeObserver) {
    this.tree = tree;
    this.observer = observer;
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

  async openTextFile(path: Path): Promise<Buffer> {
    const bufferId = await this.tree.open_text_file(path);
    return new Buffer(bufferId, this.tree, this.observer);
  }
}

export class Buffer {
  private id: BufferId;
  private tree: any;
  private observer: ChangeObserver;

  constructor(id: BufferId, tree: any, observer: ChangeObserver) {
    this.id = id;
    this.tree = tree;
    this.observer = observer;
  }

  edit(oldRanges: Range[], newText: string): Operation {
    return this.tree.edit(this.id, oldRanges, newText);
  }

  getText(): string {
    return this.tree.text(this.id);
  }

  onChange(callback: ChangeObserverCallback) {
    this.observer.onChange(this.id, callback);
  }
}
