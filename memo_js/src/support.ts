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

export type TextChangeObserverCallback = (
  changes: ReadonlyArray<Change>
) => void;
export type SelectionsChangeObserverCallback = () => void;

export class ChangeObserver {
  emitter: Emitter;

  constructor() {
    this.emitter = new Emitter();
  }

  onTextChange(
    bufferId: BufferId,
    callback: TextChangeObserverCallback
  ): Disposable {
    return this.emitter.on(`buffer-${bufferId}-text-change`, callback);
  }

  onSelectionsChange(bufferId: BufferId, callback: SelectionsChangeObserverCallback): Disposable {
    return this.emitter.on(`buffer-${bufferId}-selections-change`, callback);
  }

  textChanged(bufferId: BufferId, changes: Change[]) {
    this.emitter.emit(`buffer-${bufferId}-text-change`, changes);
  }

  selectionsChanged(bufferId: BufferId) {
    this.emitter.emit(`buffer-${bufferId}-selections-change`, {});
  }
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
