export type Tagged<BaseType, TagName> = BaseType & { __tag: TagName };
export type Oid = string;
export type Path = string;
export type BufferId = Tagged<number, "BufferId">;
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

export type ChangeObserverCallback = (changes: ReadonlyArray<Change>) => void;

export class ChangeObserver {
  private callbacks: Map<BufferId, ChangeObserverCallback[]>;

  constructor() {
    this.callbacks = new Map();
  }

  onChange(bufferId: BufferId, callback: ChangeObserverCallback) {
    let callbacks = this.callbacks.get(bufferId);
    if (!callbacks) {
      callbacks = [];
      this.callbacks.set(bufferId, callbacks);
    }
    callbacks.push(callback);
  }

  textChanged(bufferId: BufferId, changes: Change[]) {
    const callbacks = this.callbacks.get(bufferId);
    if (callbacks) {
      for (const callback of callbacks) {
        callback(changes);
      }
    }
  }
}
