use buffer_view::BufferView;
use cross_platform;
use file_finder::{FileFinderView, FileFinderViewDelegate};
use futures::{Future, Poll, Stream};
use never::Never;
use notify_cell::NotifyCell;
use notify_cell::NotifyCellObserver;
use project::{LocalProject, PathSearch, PathSearchStatus, Project, ProjectService, RemoteProject,
              TreeId};
use rpc::{self, client, server};
use serde_json;
use std::cell::Ref;
use std::cell::RefCell;
use std::rc::Rc;
use window::{View, ViewHandle, WeakViewHandle, Window};
use ForegroundExecutor;
use IntoShared;

pub trait Workspace {
    fn project(&self) -> Ref<Project>;
}

pub struct LocalWorkspace {
    project: Rc<RefCell<LocalProject>>,
}

pub struct RemoteWorkspace {
    project: Rc<RefCell<RemoteProject>>,
}

pub struct WorkspaceService {
    workspace: Rc<RefCell<LocalWorkspace>>,
}

#[derive(Serialize, Deserialize)]
pub struct ServiceState {
    project: rpc::ServiceId,
}

pub struct WorkspaceView {
    foreground: ForegroundExecutor,
    workspace: Rc<RefCell<Workspace>>,
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
    pub fn new(project: LocalProject) -> Self {
        Self {
            project: Rc::new(RefCell::new(project)),
        }
    }
}

impl Workspace for LocalWorkspace {
    fn project(&self) -> Ref<Project> {
        self.project.borrow()
    }
}

impl RemoteWorkspace {
    pub fn new(
        foreground: ForegroundExecutor,
        service: client::Service<WorkspaceService>,
    ) -> Result<Self, rpc::Error> {
        let state = service.state()?;
        let project = RemoteProject::new(foreground, service.take_service(state.project)?)?;

        eprintln!("created remote workspace");

        Ok(Self {
            project: project.into_shared(),
        })
    }
}

impl Workspace for RemoteWorkspace {
    fn project(&self) -> Ref<Project> {
        self.project.borrow()
    }
}

impl WorkspaceService {
    pub fn new(workspace: Rc<RefCell<LocalWorkspace>>) -> Self {
        Self { workspace }
    }
}

impl server::Service for WorkspaceService {
    type State = ServiceState;
    type Update = Never;
    type Request = Never;
    type Response = Never;

    fn init(&mut self, connection: &server::Connection) -> ServiceState {
        let service_handle =
            connection.add_service(ProjectService::new(self.workspace.borrow().project.clone()));

        ServiceState {
            project: service_handle.service_id(),
        }
    }
}

impl WorkspaceView {
    pub fn new(foreground: ForegroundExecutor, workspace: Rc<RefCell<Workspace>>) -> Self {
        WorkspaceView {
            workspace,
            foreground,
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
        let workspace = self.workspace.borrow();
        let project = workspace.project();
        project.search_paths(needle, max_results, include_ignored)
    }

    fn did_close(&mut self) {
        self.modal_panel = None;
        self.updates.set(());
    }

    fn did_confirm(&mut self, tree_id: TreeId, path: &cross_platform::Path, window: &mut Window) {
        let window_handle = window.handle();
        let workspace = self.workspace.borrow();
        let project = workspace.project();
        let view_handle = self.self_handle.clone();
        self.foreground
            .execute(Box::new(project.open_buffer(tree_id, path).then(
                move |result| {
                    window_handle.map(|window| match result {
                        Ok(buffer) => {
                            let mut buffer_view = BufferView::new(buffer);
                            buffer_view.set_line_height(20.0);
                            let buffer_view = window.add_view(buffer_view);
                            buffer_view.focus().unwrap();
                            if let Some(view_handle) = view_handle {
                                view_handle.map(|view| {
                                    view.center_pane = Some(buffer_view);
                                    view.modal_panel = None;
                                    view.updates.set(());
                                });
                            }
                        }
                        Err(error) => {
                            unimplemented!("Error handling for open_buffer: {:?}", error);
                        }
                    });
                    Ok(())
                },
            )))
            .unwrap();
    }
}

impl Stream for WorkspaceView {
    type Item = ();
    type Error = ();

    fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
        self.updates.poll()
    }
}
