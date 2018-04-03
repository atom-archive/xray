use fs;
use workspace::WorkspaceHandle;

pub struct Peer {
    workspaces: Vec<WorkspaceHandle>,
}

impl Peer {
    pub fn new() -> Self {
        Self {
            workspaces: Vec::new()
        }
    }

    pub fn open_workspace(&mut self, roots: Vec<Box<fs::Tree>>) -> WorkspaceHandle {
        let workspace = WorkspaceHandle::new(roots);
        self.workspaces.push(workspace.clone());
        workspace
    }
}
