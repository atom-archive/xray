use buffer::{self, BufferId};
use buffer_view::{BufferView, BufferViewDelegate};
use cross_platform;
use discussion::{Discussion, DiscussionService, DiscussionView, DiscussionViewDelegate};
use file_finder::{FileFinderView, FileFinderViewDelegate};
use futures::{Future, Poll, Stream};
use never::Never;
use notify_cell::NotifyCell;
use notify_cell::NotifyCellObserver;
use project::{LocalProject, PathSearch, PathSearchStatus, Project, ProjectService, RemoteProject,
              TreeId};
use rpc::{self, client, server};
use serde_json;
use std::cell::{Ref, RefCell, RefMut};
use std::ops::Range;
use std::rc::Rc;
use window::{View, ViewHandle, WeakViewHandle, Window};
use ForegroundExecutor;
use IntoShared;
use UserId;

pub trait Workspace {
    fn user_id(&self) -> UserId;
    fn project(&self) -> Ref<Project>;
    fn project_mut(&self) -> RefMut<Project>;
    fn discussion(&self) -> &Rc<RefCell<Discussion>>;
}

pub struct LocalWorkspace {
    next_user_id: UserId,
    user_id: UserId,
    discussion: Rc<RefCell<Discussion>>,
    project: Rc<RefCell<LocalProject>>,
}

pub struct RemoteWorkspace {
    user_id: UserId,
    project: Rc<RefCell<RemoteProject>>,
    discussion: Rc<RefCell<Discussion>>,
}

pub struct WorkspaceService {
    workspace: Rc<RefCell<LocalWorkspace>>,
}

#[derive(Serialize, Deserialize)]
pub struct ServiceState {
    user_id: UserId,
    project: rpc::ServiceId,
    discussion: rpc::ServiceId,
}

pub struct WorkspaceView {
    foreground: ForegroundExecutor,
    workspace: Rc<RefCell<Workspace>>,
    active_buffer_view: Option<WeakViewHandle<BufferView<WorkspaceView>>>,
    center_pane: Option<ViewHandle>,
    modal: Option<ViewHandle>,
    left_panel: Option<ViewHandle>,
    updates: NotifyCell<()>,
    self_handle: Option<WeakViewHandle<WorkspaceView>>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Anchor {
    buffer_id: BufferId,
    range: Range<buffer::Anchor>,
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum WorkspaceViewAction {
    ToggleFileFinder,
}

impl LocalWorkspace {
    pub fn new(project: LocalProject) -> Self {
        Self {
            user_id: 0,
            next_user_id: 1,
            project: project.into_shared(),
            discussion: Discussion::new(0).into_shared(),
        }
    }
}

impl Workspace for LocalWorkspace {
    fn user_id(&self) -> UserId {
        self.user_id
    }

    fn project(&self) -> Ref<Project> {
        self.project.borrow()
    }

    fn project_mut(&self) -> RefMut<Project> {
        self.project.borrow_mut()
    }

    fn discussion(&self) -> &Rc<RefCell<Discussion>> {
        &self.discussion
    }
}

impl RemoteWorkspace {
    pub fn new(
        foreground: ForegroundExecutor,
        service: client::Service<WorkspaceService>,
    ) -> Result<Self, rpc::Error> {
        let state = service.state()?;
        let project = RemoteProject::new(foreground.clone(), service.take_service(state.project)?)?;
        let discussion = Discussion::remote(
            foreground,
            state.user_id,
            service.take_service(state.discussion)?,
        )?;

        Ok(Self {
            user_id: state.user_id,
            project: project.into_shared(),
            discussion,
        })
    }
}

impl Workspace for RemoteWorkspace {
    fn user_id(&self) -> UserId {
        self.user_id
    }

    fn project(&self) -> Ref<Project> {
        self.project.borrow()
    }

    fn project_mut(&self) -> RefMut<Project> {
        self.project.borrow_mut()
    }

    fn discussion(&self) -> &Rc<RefCell<Discussion>> {
        &self.discussion
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
        let mut workspace = self.workspace.borrow_mut();
        let user_id = workspace.next_user_id;
        workspace.next_user_id += 1;
        ServiceState {
            user_id,
            project: connection
                .add_service(ProjectService::new(workspace.project.clone()))
                .service_id(),
            discussion: connection
                .add_service(DiscussionService::new(
                    user_id,
                    workspace.discussion.clone(),
                ))
                .service_id(),
        }
    }
}

impl WorkspaceView {
    pub fn new(foreground: ForegroundExecutor, workspace: Rc<RefCell<Workspace>>) -> Self {
        WorkspaceView {
            workspace,
            foreground,
            active_buffer_view: None,
            center_pane: None,
            modal: None,
            left_panel: None,
            updates: NotifyCell::new(()),
            self_handle: None,
        }
    }

    fn toggle_file_finder(&mut self, window: &mut Window) {
        if self.modal.is_some() {
            self.modal = None;
        } else {
            let delegate = self.self_handle.as_ref().cloned().unwrap();
            let view = window.add_view(FileFinderView::new(delegate));
            view.focus().unwrap();
            self.modal = Some(view);
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
            "center_pane": self.center_pane.as_ref().map(|view_handle| view_handle.view_id),
            "modal": self.modal.as_ref().map(|view_handle| view_handle.view_id),
            "left_panel": self.left_panel.as_ref().map(|view_handle| view_handle.view_id)
        })
    }

    fn will_mount(&mut self, window: &mut Window, view_handle: WeakViewHandle<Self>) {
        self.self_handle = Some(view_handle.clone());
        self.left_panel = Some(window.add_view(DiscussionView::new(
            self.workspace.borrow().discussion().clone(),
            view_handle,
        )))
    }

    fn dispatch_action(&mut self, action: serde_json::Value, window: &mut Window) {
        match serde_json::from_value(action) {
            Ok(WorkspaceViewAction::ToggleFileFinder) => self.toggle_file_finder(window),
            _ => eprintln!("Unrecognized action"),
        }
    }
}

impl BufferViewDelegate for WorkspaceView {
    fn set_active_buffer_view(&mut self, handle: WeakViewHandle<BufferView<Self>>) {
        self.active_buffer_view = Some(handle);
    }
}

impl DiscussionViewDelegate for WorkspaceView {
    fn anchor(&self) -> Option<Anchor> {
        self.active_buffer_view.as_ref().and_then(|handle| {
            handle.map(|buffer_view| Anchor {
                buffer_id: buffer_view.buffer_id(),
                range: buffer_view.selections().last().unwrap().anchor_range(),
            })
        })
    }

    fn jump(&self, anchor: &Anchor) {
        unimplemented!()
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
        self.modal = None;
        self.updates.set(());
    }

    fn did_confirm(&mut self, tree_id: TreeId, path: &cross_platform::Path, window: &mut Window) {
        let window_handle = window.handle();
        let workspace = self.workspace.borrow();
        let project = workspace.project_mut();
        let user_id = workspace.user_id();
        let view_handle = self.self_handle.clone();
        self.foreground
            .execute(Box::new(project.open_buffer(tree_id, path).then(
                move |result| {
                    window_handle.map(|window| match result {
                        Ok(buffer) => {
                            if let Some(view_handle) = view_handle {
                                let mut buffer_view =
                                    BufferView::new(buffer, user_id, Some(view_handle.clone()));
                                buffer_view.set_line_height(20.0);
                                let buffer_view = window.add_view(buffer_view);
                                buffer_view.focus().unwrap();
                                view_handle.map(|view| {
                                    view.center_pane = Some(buffer_view);
                                    view.modal = None;
                                    view.updates.set(());
                                });
                            }
                        }
                        Err(error) => {
                            eprintln!("Error opening buffer {:?}", error);
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
