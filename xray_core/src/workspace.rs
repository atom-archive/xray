use buffer_view::BufferView;
use file_finder::{FileFinderView, FileFinderViewDelegate};
use fs;
use futures::{Poll, Stream};
use notify_cell::NotifyCell;
use notify_cell::NotifyCellObserver;
use project::{PathSearch, PathSearchStatus, Project, TreeId};
use serde_json;
use std::cell::{Ref, RefCell};
use std::path::Path;
use std::rc::Rc;
use window::{View, ViewHandle, WeakViewHandle, Window};

pub trait Workspace {
    fn search_paths(
        &self,
        needle: &str,
        max_results: usize,
        include_ignored: bool,
    ) -> (PathSearch, NotifyCellObserver<PathSearchStatus>);
    fn project(&self) -> Ref<Project>;
}

#[derive(Clone)]
pub struct LocalWorkspace(Rc<RefCell<LocalWorkspaceState>>);

struct LocalWorkspaceState {
    project: Project,
}

pub struct WorkspaceView {
    workspace: Box<Workspace>,
    modal_panel: Option<ViewHandle>,
    center_pane: Option<ViewHandle>,
    updates: NotifyCell<()>,
    self_handle: Option<WeakViewHandle<WorkspaceView>>,
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum WorkspaceViewAction {
    ToggleFileFinder,
}

impl LocalWorkspace {
    pub fn new<T: 'static + fs::LocalTree>(roots: Vec<T>) -> Self {
        LocalWorkspace(Rc::new(RefCell::new(LocalWorkspaceState {
            project: Project::new(roots),
        })))
    }
}

impl Workspace for LocalWorkspace {
    fn search_paths(
        &self,
        needle: &str,
        max_results: usize,
        include_ignored: bool,
    ) -> (PathSearch, NotifyCellObserver<PathSearchStatus>) {
        self.0
            .borrow()
            .project
            .search_paths(needle, max_results, include_ignored)
    }

    fn project(&self) -> Ref<Project> {
        Ref::map(self.0.borrow(), |state| &state.project)
    }
}

impl WorkspaceView {
    pub fn new<T: 'static + Workspace>(workspace: T) -> Self {
        WorkspaceView {
            workspace: Box::new(workspace),
            modal_panel: None,
            center_pane: None,
            updates: NotifyCell::new(()),
            self_handle: None,
        }
    }

    fn toggle_file_finder(&mut self, window: &mut Window) {
        if self.modal_panel.is_some() {
            self.modal_panel = None;
        } else {
            let delegate = self.self_handle.as_ref().cloned().unwrap();
            let view = window.add_view(FileFinderView::new(delegate));
            view.focus().unwrap();
            self.modal_panel = Some(view);
        }
        self.updates.set(());
    }
}

impl View for WorkspaceView {
    fn component_name(&self) -> &'static str {
        "Workspace"
    }

    fn render(&self) -> serde_json::Value {
        json!({
            "modal": self.modal_panel.as_ref().map(|view_handle| view_handle.view_id),
            "center_pane": self.center_pane.as_ref().map(|view_handle| view_handle.view_id)
        })
    }

    fn will_mount(&mut self, _window: &mut Window, view_handle: WeakViewHandle<Self>) {
        self.self_handle = Some(view_handle);
    }

    fn dispatch_action(&mut self, action: serde_json::Value, window: &mut Window) {
        match serde_json::from_value(action) {
            Ok(WorkspaceViewAction::ToggleFileFinder) => self.toggle_file_finder(window),
            _ => eprintln!("Unrecognized action"),
        }
    }
}

impl FileFinderViewDelegate for WorkspaceView {
    fn search_paths(
        &self,
        needle: &str,
        max_results: usize,
        include_ignored: bool,
    ) -> (PathSearch, NotifyCellObserver<PathSearchStatus>) {
        self.workspace
            .search_paths(needle, max_results, include_ignored)
    }

    fn did_close(&mut self) {
        self.modal_panel = None;
        self.updates.set(());
    }

    fn did_confirm(&mut self, tree_id: TreeId, path: &Path, window: &mut Window) {
        match self.workspace.project().open_buffer(tree_id, path) {
            Ok(buffer) => {
                let mut buffer_view = BufferView::new(Rc::new(RefCell::new(buffer)));
                buffer_view.set_line_height(20.0);
                let buffer_view = window.add_view(buffer_view);
                buffer_view.focus().unwrap();
                self.center_pane = Some(buffer_view);
                self.modal_panel = None;
                self.updates.set(());
            }
            Err(error) => {
                unimplemented!("Error handling for open_buffer: {:?}", error);
            }
        }
    }
}

impl Stream for WorkspaceView {
    type Item = ();
    type Error = ();

    fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
        self.updates.poll()
    }
}
