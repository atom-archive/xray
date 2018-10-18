export { BaseEntry, GitProvider, FileType, Oid } from './support';
import { GitProvider, GitProviderWrapper, Oid } from './support';

let memo: any;

export async function init() {
    memo = await import("../dist/memo_js");
    memo.StreamToAsyncIterator.prototype[Symbol.asyncIterator] = function () {
        return this;
    }
    return { WorkTree };
}

// export interface GitProvider = GitProvider;

type Tagged<BaseType, TagName> = BaseType & { __tag: TagName };

export type FileId = Tagged<string, "FileId">;
export type BufferId = Tagged<string, "BufferId">;
export type Version = Tagged<object, "Version">;
export type Operation = Tagged<string, "Operation">;

export class WorkTree {
    private tree: any;

    static create(replicaId: number, base: Oid, startOps: ReadonlyArray<Operation>, git: GitProvider): [WorkTree, AsyncIterable<Operation>] {
        const result = memo.WorkTree.new(new GitProviderWrapper(git), { replica_id: replicaId, base, start_ops: startOps });
        return [new WorkTree(result.tree()), result.operations()];
    }

    constructor(tree: any) {
        this.tree = tree;
    }
}