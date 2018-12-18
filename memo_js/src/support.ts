export type Tagged<BaseType, TagName> = BaseType & { __tag: TagName };
export type Oid = string;
export type Path = string;
export type ReplicaId = Tagged<string, "ReplicaId">;
export type BufferId = Tagged<number, "BufferId">;
export type SelectionSetId = Tagged<number, "SelectionSetId">;
export type Point = { row: number; column: number };
export type Range = { start: Point; end: Point };
export type Change = Range & { text: string };

export interface BaseEntry {
  readonly depth: number;
  readonly name: string;
  readonly type: FileType;
}

export enum FileType {
  Directory = "Directory",
  Text = "Text"
}

export interface GitProvider {
  baseEntries(oid: Oid): AsyncIterable<BaseEntry>;
  baseText(oid: Oid, path: Path): Promise<string>;
}

export interface SelectionRanges {
  local: Map<SelectionSetId, Array<Range>>;
  remote: Map<ReplicaId, Array<Array<Range>>>;
}

interface MemoSelectionRanges {
  local: { [setId: number]: Array<Range> };
  remote: { [replicaId: string]: Array<Array<Range>> };
}

export class GitProviderWrapper {
  private git: GitProvider;

  constructor(git: GitProvider) {
    this.git = git;
  }

  baseEntries(oid: Oid): AsyncIteratorWrapper<BaseEntry> {
    return new AsyncIteratorWrapper(
      this.git.baseEntries(oid)[Symbol.asyncIterator]()
    );
  }

  baseText(oid: Oid, path: Path): Promise<string> {
    return this.git.baseText(oid, path);
  }
}

export class AsyncIteratorWrapper<T> {
  private iterator: AsyncIterator<T>;

  constructor(iterator: AsyncIterator<T>) {
    this.iterator = iterator;
  }

  next(): Promise<IteratorResult<T>> {
    return this.iterator.next();
  }
}

export type ChangeObserverCallback = (
  change: {
    textChanges: ReadonlyArray<Change>;
    selectionRanges: SelectionRanges;
  }
) => void;

export class ChangeObserver {
  emitter: Emitter;

  constructor() {
    this.emitter = new Emitter();
  }

  onChange(bufferId: BufferId, callback: ChangeObserverCallback): Disposable {
    return this.emitter.on(`buffer-${bufferId}-change`, callback);
  }

  changed(
    bufferId: BufferId,
    textChanges: Change[],
    selectionRanges: MemoSelectionRanges
  ) {
    this.emitter.emit(`buffer-${bufferId}-change`, {
      textChanges,
      selectionRanges: fromMemoSelectionRanges(selectionRanges)
    });
  }
}

export function fromMemoSelectionRanges(
  ranges: MemoSelectionRanges
): SelectionRanges {
  const local = new Map();
  for (const setId in ranges.local) {
    local.set(setId, ranges.local[setId]);
  }

  const remote = new Map();
  for (const replicaId in ranges.remote) {
    remote.set(replicaId, ranges.remote[replicaId]);
  }

  return { local, remote };
}

export interface Disposable {
  dispose(): void;
}

export class CompositeDisposable implements Disposable {
  private disposables: Disposable[];
  private disposed: boolean;

  constructor() {
    this.disposables = [];
    this.disposed = false;
  }

  add(disposable: Disposable) {
    this.disposables.push(disposable);
  }

  dispose() {
    if (!this.disposed) {
      this.disposed = true;
      for (const disposable of this.disposables) {
        disposable.dispose();
      }
    }
  }
}

export type EmitterCallback = (params: any) => void;
export class Emitter {
  private callbacks: Map<string, EmitterCallback[]>;

  constructor() {
    this.callbacks = new Map();
  }

  emit(eventName: string, params: any) {
    const callbacks = this.callbacks.get(eventName);
    if (callbacks) {
      for (const callback of callbacks) {
        callback(params);
      }
    }
  }

  on(eventName: string, callback: EmitterCallback): Disposable {
    let callbacks = this.callbacks.get(eventName);
    if (!callbacks) {
      callbacks = [];
      this.callbacks.set(eventName, callbacks);
    }
    callbacks.push(callback);
    return {
      dispose: () => {
        if (callbacks) {
          const callbackIndex = callbacks.indexOf(callback);
          if (callbackIndex >= 0) callbacks.splice(callbackIndex, 1);
        }
      }
    };
  }
}
