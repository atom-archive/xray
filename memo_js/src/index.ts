let server: any;

export async function init() {
  const memo = await import("../dist/memo_wasm");
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

type FileId = string;
type BufferId = string;
type Version = object;
type Operation = string;

enum FileType {
  Directory = "Directory",
  File = "File"
}

enum FileStatus {
  New = "New",
  Renamed = "Renamed",
  Removed = "Removed",
  Modified = "Modified",
  Unchanged = "Unchanged"
}

interface BaseEntry {
  depth: number;
  name: string;
  type: FileType;
}

interface Entry {
  depth: number;
  fileId: FileId;
  type: FileType;
  name: string;
  status: FileStatus;
  visible: boolean;
}

class WorkTree {
  private static rootFileId: FileId;
  private id: number;

  static getRootFileId(): FileId {
    if (!WorkTree.rootFileId) {
      WorkTree.rootFileId = request({ type: "GetRootFileId" }).file_id;
    }
    return WorkTree.rootFileId;
  }

  constructor(replicaId: number) {
    this.id = request({
      type: "CreateWorkTree",
      replica_id: replicaId
    }).tree_id;
  }

  getVersion(): Version {
    return request({ tree_id: this.id, type: "GetVersion" }).version;
  }

  appendBaseEntries(baseEntries: [BaseEntry]): [Operation] {
    return request({
      type: "AppendBaseEntries",
      tree_id: this.id,
      entries: baseEntries
    }).operations;
  }

  applyOps(operations: [Operation]): [Operation] {
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

  openTextFile(fileId: FileId, baseText: string): BufferId {
    const response = request({
      type: "OpenTextFile",
      tree_id: this.id,
      file_id: fileId,
      base_text: baseText
    });
    return response.buffer_id;
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

  edit(
    bufferId: BufferId,
    ranges: [{ start: number; end: number }],
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
  ): [{ start: number; end: number; text: string }] {
    return request({
      type: "ChangesSince",
      tree_id: this.id,
      buffer_id: bufferId,
      version
    }).changes;
  }

  getText(bufferId: BufferId): string {
    return request({
      type: "GetText",
      tree_id: this.id,
      buffer_id: bufferId
    }).text;
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

  entries(options?: {showDeleted?: boolean,  descendInto?: [FileId]}): Entry {
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
}
