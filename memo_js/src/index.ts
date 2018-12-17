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
  ChangeObserver,
  Disposable,
  GitProvider,
  GitProviderWrapper,
  FileType,
  Oid,
  Path,
  Range,
  SelectionsChangeObserverCallback,
  Tagged,
  TextChangeObserverCallback
} from "./support";

let memo: any;

async function init() {
  if (!memo) {
    memo = await import("../dist/memo_js");
    memo.StreamToAsyncIterator.prototype[Symbol.asyncIterator] = function() {
      return this;
    };
  }
}

export type Version = Tagged<Uint8Array, "Version">;
export type Operation = Tagged<Uint8Array, "Operation">;
export type EpochId = Tagged<Uint8Array, "EpochId">;
export type ReplicaId = Tagged<string, "ReplicaId">;
export type SelectionSetId = Tagged<number, "SelectionSetId">;
export interface OperationEnvelope {
  epochId(): EpochId;
  epochTimestamp(): number;
  epochReplicaId(): ReplicaId;
  epochHead(): null | Oid;
  operation(): Operation;
}

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

export interface Selections {
  local: Map<SelectionSetId, Array<Range>>;
  remote: Map<ReplicaId, Array<Array<Range>>>;
}

export class WorkTree {
  private tree: any;
  private observer: ChangeObserver;
  private buffers: Map<BufferId, Buffer> = new Map();

  static async create(
    replicaId: string,
    base: Oid | null,
    startOps: ReadonlyArray<Operation>,
    git: GitProvider
  ): Promise<[WorkTree, AsyncIterable<OperationEnvelope>]> {
    await init();

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

  version(): Version {
    return this.tree.version();
  }

  hasObserved(version: Version): boolean {
    return this.tree.observed(version);
  }

  head(): null | Oid {
    return this.tree.head();
  }

  reset(base: Oid | null): AsyncIterable<OperationEnvelope> {
    return this.tree.reset(base);
  }

  applyOps(ops: Operation[]): AsyncIterable<OperationEnvelope> {
    return this.tree.apply_ops(ops);
  }

  createFile(path: Path, fileType: FileType): OperationEnvelope {
    return this.tree.create_file(path, fileType);
  }

  rename(oldPath: Path, newPath: Path): OperationEnvelope {
    return this.tree.rename(oldPath, newPath);
  }

  remove(path: Path): OperationEnvelope {
    return this.tree.remove(path);
  }

  exists(path: Path): boolean {
    return this.tree.exists(path);
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
    let buffer = this.buffers.get(bufferId);
    if (!buffer) {
      buffer = new Buffer(bufferId, this.tree, this.observer);
      this.buffers.set(bufferId, buffer);
    }
    return buffer;
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

  edit(oldRanges: Range[], newText: string): OperationEnvelope {
    return this.tree.edit(this.id, oldRanges, newText);
  }

  addSelectionSet(ranges: Range[]): [SelectionSetId, OperationEnvelope] {
    const result = this.tree.add_selection_set(this.id, ranges);
    return [result.set_id(), result.operation()];
  }

  replaceSelectionSet(id: SelectionSetId, ranges: Range[]): OperationEnvelope {
    return this.tree.replace_selection_set(this.id, id, ranges);
  }

  removeSelectionSet(id: SelectionSetId): OperationEnvelope {
    return this.tree.remove_selection_set(this.id, id);
  }

  getPath(): string | null {
    return this.tree.path(this.id);
  }

  getText(): string {
    return this.tree.text(this.id);
  }

  getSelections(): Selections {
    const selections = this.tree.selections(this.id);

    const local = new Map();
    for (const setId in selections.local) {
      local.set(setId, selections.local[setId]);
    }

    const remote = new Map();
    for (const replicaId in selections.remote) {
      remote.set(replicaId, selections.remote[replicaId]);
    }

    return { local, remote };
  }

  onTextChange(callback: TextChangeObserverCallback): Disposable {
    return this.observer.onTextChange(this.id, callback);
  }

  onSelectionsChange(callback: SelectionsChangeObserverCallback): Disposable {
    return this.observer.onSelectionsChange(this.id, callback);
  }

  getDeferredOperationCount(): number {
    return this.tree.buffer_deferred_ops_len(this.id);
  }
}
