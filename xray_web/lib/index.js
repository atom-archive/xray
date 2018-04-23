const FileFinder = require("./file_finder");
const ViewRegistry = require("./view_registry");
const Workspace = require("./workspace");
const TextEditorView = require("./text_editor/text_editor");

exports.buildViewRegistry = function buildViewRegistry(client) {
  const viewRegistry = new ViewRegistry({
    onAction: action => {
      action.type = "Action";
      client.sendMessage(action);
    }
  });
  viewRegistry.addComponent("Workspace", Workspace);
  viewRegistry.addComponent("FileFinder", FileFinder);
  viewRegistry.addComponent("BufferView", TextEditorView);
  return viewRegistry;
};

exports.App = require("./app");
