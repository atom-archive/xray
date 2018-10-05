let server: any;

export async function init() {
  const memo = await import("../dist/memo_js");
  if (!server) {
    server = memo.Server.new();
  }
  return { WorkTree };
}

function request(req: any) {
  const response = server.request(req);
  if (response.type == "Error") {
    throw new Error(response.message);
  } else {
    return response;
  }
}

export type FileId = string;
export type BufferId = string;
export type Version = object;
export type Operation = string;

export enum FileType {
  Directory = "Directory",
  Text = "Text"
}

export enum FileStatus {
  New = "New",
  Renamed = "Renamed",
  Removed = "Removed",
  Modified = "Modified",
  Unchanged = "Unchanged"
}

export interface BaseEntry {
  readonly depth: number;
  readonly name: string;
  readonly type: FileType;
}

export interface Entry {
  readonly depth: number;
  readonly fileId: FileId;
  readonly type: FileType;
  readonly name: string;
  readonly status: FileStatus;
  readonly visible: boolean;
}

export interface Point {
  readonly row: number;
  readonly column: number;
}

export interface Range {
  readonly start: Point;
  readonly end: Point;
}

export interface RangeWithText extends Range {
  readonly text: string;
}

export class WorkTree {
  private static rootFileId: FileId;
  private id: number;

  static getRootFileId(): FileId {
    if (!WorkTree.rootFileId) {
      WorkTree.rootFileId = request({ type: "GetRootFileId" }).file_id;
    }
    return WorkTree.rootFileId;
  }

  constructor(replicaId: number) {
    if (replicaId <= 0) {
      throw new Error("Replica id must be positive");
    }

    this.id = request({
      type: "CreateWorkTree",
      replica_id: replicaId
    }).tree_id;
  }

  getVersion(): Version {
    return request({ tree_id: this.id, type: "GetVersion" }).version;
  }

  appendBaseEntries(
    baseEntries: ReadonlyArray<BaseEntry>
  ): ReadonlyArray<Operation> {
    return request({
      type: "AppendBaseEntries",
      tree_id: this.id,
      entries: baseEntries
    }).operations;
  }

  applyOps(operations: ReadonlyArray<Operation>): ReadonlyArray<Operation> {
    const response = request({
      type: "ApplyOperations",
      tree_id: this.id,
      operations
    });
    return response.operations;
  }

  newTextFile(): { fileId: FileId; operation: Operation } {
    const { file_id, operation } = request({
      type: "NewTextFile",
      tree_id: this.id
    });
    return { fileId: file_id, operation };
  }

  createDirectory(
    parentId: FileId,
    name: string
  ): { fileId: FileId; operation: Operation } {
    const { file_id, operation } = request({
      type: "CreateDirectory",
      tree_id: this.id,
      parent_id: parentId,
      name
    });

    return { fileId: file_id, operation };
  }

  rename(fileId: FileId, newParentId: FileId, newName: string): Operation {
    return request({
      type: "Rename",
      tree_id: this.id,
      file_id: fileId,
      new_parent_id: newParentId,
      new_name: newName
    }).operation;
  }

  remove(fileId: FileId): Operation {
    return request({
      type: "Remove",
      tree_id: this.id,
      file_id: fileId
    }).operation;
  }

  fileIdForPath(path: string): FileId {
    return request({
      type: "FileIdForPath",
      tree_id: this.id,
      path
    }).file_id;
  }

  pathForFileId(id: FileId): string {
    return request({
      type: "PathForFileId",
      tree_id: this.id,
      file_id: id
    }).path;
  }

  entries(options?: {
    showDeleted?: boolean;
    descendInto?: ReadonlyArray<FileId>;
  }): ReadonlyArray<Entry> {
    let showDeleted, descendInto;
    if (options) {
      showDeleted = options.showDeleted || false;
      descendInto = options.descendInto || null;
    } else {
      showDeleted = false;
      descendInto = null;
    }

    return request({
      type: "Entries",
      tree_id: this.id,
      show_deleted: showDeleted,
      descend_into: descendInto
    }).entries;
  }

  openTextFile(fileId: FileId, baseText: string): BufferId {
    const response = request({
      type: "OpenTextFile",
      tree_id: this.id,
      file_id: fileId,
      base_text: baseText
    });
    return response.buffer_id;
  }

  getText(bufferId: BufferId): string {
    return request({
      type: "GetText",
      tree_id: this.id,
      buffer_id: bufferId
    }).text;
  }

  edit(
    bufferId: BufferId,
    ranges: ReadonlyArray<Range>,
    newText: string
  ): Operation {
    const response = request({
      type: "Edit",
      tree_id: this.id,
      buffer_id: bufferId,
      ranges,
      new_text: newText
    });
    return response.operation;
  }

  changesSince(
    bufferId: BufferId,
    version: Version
  ): ReadonlyArray<RangeWithText> {
    return request({
      type: "ChangesSince",
      tree_id: this.id,
      buffer_id: bufferId,
      version
    }).changes;
  }
}
