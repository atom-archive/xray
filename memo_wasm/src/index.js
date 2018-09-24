let server;

const memoImportPromise = import("../dist/memo_wasm");

export async function initialize() {
  const memo = await memoImportPromise;
  if (!server) {
    server = memo.Server.new();
  }
  return { WorkTree };
}

function request(req) {
  const response = server.request(req);
  if (response.type == "Error") {
    throw new Error(response.message);
  } else {
    return response;
  }
}

class WorkTree {
  static getRootFileId() {
    if (!WorkTree.rootFileId) {
      WorkTree.rootFileId = request({ type: "GetRootFileId" }).file_id;
    }
    return WorkTree.rootFileId;
  }

  constructor(replicaId) {
    this.id = request({
      type: "CreateWorkTree",
      replica_id: replicaId
    }).tree_id;
  }

  getVersion() {
    return request({ tree_id: this.id, type: "GetVersion" }).version;
  }

  appendBaseEntries(baseEntries) {
    request({
      type: "AppendBaseEntries",
      tree_id: this.id,
      entries: baseEntries
    });
  }

  applyOps(operations) {
    const response = request({
      type: "ApplyOperations",
      tree_id: this.id,
      operations
    });
    return response.operations;
  }

  newTextFile() {
    const { file_id, operation } = request({
      type: "NewTextFile",
      tree_id: this.id
    });
    return { fileId: file_id, operation };
  }

  createDirectory(parentId, name) {
    const { file_id, operation } = request({
      type: "CreateDirectory",
      tree_id: this.id,
      parent_id: parentId,
      name
    });

    return { fileId: file_id, operation };
  }

  openTextFile(fileId, baseText) {
    const response = request({
      type: "OpenTextFile",
      tree_id: this.id,
      file_id: fileId,
      base_text: baseText
    });
    return response.buffer_id;
  }

  rename(fileId, newParentId, newName) {
    return request({
      type: "Rename",
      tree_id: this.id,
      file_id: fileId,
      new_parent_id: newParentId,
      new_name: newName
    }).operation;
  }

  remove(fileId) {
    return request({
      type: "Remove",
      tree_id: this.id,
      file_id: fileId
    }).operation;
  }

  edit(bufferId, ranges, newText) {
    const response = request({
      type: "Edit",
      tree_id: this.id,
      buffer_id: bufferId,
      ranges,
      new_text: newText
    });
    return response.operation;
  }

  changesSince(bufferId, version) {
    return request({
      type: "ChangesSince",
      tree_id: this.id,
      buffer_id: bufferId,
      version
    }).changes;
  }

  getText(bufferId) {
    return request({
      type: "GetText",
      tree_id: this.id,
      buffer_id: bufferId
    }).text;
  }

  fileIdForPath(path) {
    return request({
      type: "FileIdForPath",
      tree_id: this.id,
      path
    }).file_id;
  }

  pathForFileId(id) {
    return request({
      type: "PathForFileId",
      tree_id: this.id,
      file_id: id
    }).path;
  }

  entries(descendInto = []) {
    return request({
      type: "Entries",
      tree_id: this.id,
      descend_into: descendInto
    }).entries;
  }
}
