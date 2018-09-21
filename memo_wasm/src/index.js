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
}
