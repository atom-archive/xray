export type Oid = string;

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
}

export class GitProviderWrapper {
    private git: GitProvider;

    constructor(git: GitProvider) {
        this.git = git;
    }

    baseEntries(oid: Oid): AsyncIteratorWrapper<BaseEntry> {
        return new AsyncIteratorWrapper(this.git.baseEntries(oid)[Symbol.asyncIterator]());
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