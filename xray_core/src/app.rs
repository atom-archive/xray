use std::collections::HashMap;

// Server

struct App<E>
    where E: Executor<Box<Future<Item = (), Error = ()>>>
{
    workspace_views: HashMap<usize, WorkspaceView>,
    executor: E
}

// Core

trait View {
    fn type() -> &str;
    fn id() -> usize;
    fn render() -> serde_json::Value;
    fn handle_action(serde_json::Value);
    fn changes() -> &NotifyCell<()>;
}

struct WorkspaceView<E>
    where E: Executor<Box<Future<Item = (), Error = ()>>>
{
    modal: Option<Box<View>>,
    executor: Option<&E>
    fn changes() -> Stream<>
}

struct Workspace {
    
}

struct Project {
    roots: Vec<FsEntry>
}

enum FsEntry {
    Directory {
        name: OsString,
        entries: Vec<FileSystemEntry>,
        is_symlink: bool
    },
    File {
        name: OsString
        is_symlink: bool
    }
}

struct Document {
    
}

struct DocumentView {
    
}