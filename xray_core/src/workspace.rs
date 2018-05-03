use buffer::{self, Buffer, BufferId};
use buffer_view::{BufferView, BufferViewDelegate};
use cross_platform;
use discussion::{Discussion, DiscussionService, DiscussionView, DiscussionViewDelegate};
use file_finder::{FileFinderView, FileFinderViewDelegate};
use futures::prelude::*;
use notify_cell::NotifyCell;
use notify_cell::NotifyCellObserver;
use project::{self, LocalProject, PathSearch, PathSearchStatus, Project, ProjectService,
              RemoteProject, TreeId};
use rpc::{self, client, server};
use serde_json;
use std::cell::{Ref, RefCell, RefMut};
use std::ops::Range;
use std::rc::Rc;
use window::{View, ViewHandle, WeakViewHandle, WeakWindowHandle, Window};
use Executor;
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
    executor: Rc<Executor>,
    workspace: Rc<RefCell<Workspace>>,
    active_buffer_view: Option<WeakViewHandle<BufferView>>,
    center_pane: Option<ViewHandle>,
    modal: Option<ViewHandle>,
    left_panel: Option<ViewHandle>,
    updates: NotifyCell<()>,
    self_handle: Option<WeakViewHandle<WorkspaceView>>,
    window_handle: Option<WeakWindowHandle>,
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
    ToggleDiscussion,
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
        executor: Rc<Executor>,
        service: client::Service<WorkspaceService>,
    ) -> Result<Self, rpc::Error> {
        let state = service.state()?;
        let project = RemoteProject::new(executor.clone(), service.take_service(state.project)?)?;
        let discussion = Discussion::remote(
            executor,
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
    type Update = ();
    type Request = ();
    type Response = ();

    fn init(&mut self, _cx: &mut task::Context, connection: &server::Connection) -> ServiceState {
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
    pub fn new(executor: Rc<Executor>, workspace: Rc<RefCell<Workspace>>) -> Self {
        WorkspaceView {
            workspace,
            executor,
            active_buffer_view: None,
            center_pane: None,
            modal: None,
            left_panel: None,
            updates: NotifyCell::new(()),
            self_handle: None,
            window_handle: None,
        }
    }

    fn toggle_file_finder(&mut self, window: &mut Window) {
        if self.modal.is_some() {
            self.modal = None;
        } else {
            let delegate = self.self_handle.as_ref().cloned().unwrap();
            let view = window.add_view(FileFinderView::new(self.executor.clone(), delegate));
            view.focus().unwrap();
            self.modal = Some(view);
        }
        self.updates.set(());
    }

    fn toggle_discussion(&mut self, window: &mut Window) {
        if self.left_panel.is_some() {
            self.left_panel = None;
        } else {
            let delegate = self.self_handle.as_ref().cloned().unwrap();
            self.left_panel = Some(window.add_view(DiscussionView::new(
                self.workspace.borrow().discussion().clone(),
                delegate,
            )));
        }
        self.updates.set(());
    }

    fn open_buffer<T>(&self, buffer: T, selected_range: Option<Range<buffer::Anchor>>)
    where
        T: 'static + Future<Item = Rc<RefCell<Buffer>>, Error = project::OpenError>,
    {
        if let Some(window_handle) = self.window_handle.clone() {
            let user_id = self.workspace.borrow().user_id();
            let view_handle = self.self_handle.clone();
            self.executor
                .spawn_foreground(Box::new(buffer.then(move |result| {
                    window_handle.map(|window| match result {
                        Ok(buffer) => {
                            if let Some(view_handle) = view_handle {
                                let mut buffer_view =
                                    BufferView::new(buffer, user_id, Some(view_handle.clone()));
                                buffer_view.set_line_height(20.0);
                                if let Some(selected_range) = selected_range {
                                    if let Err(error) =
                                        buffer_view.set_selected_anchor_range(selected_range)
                                    {
                                        eprintln!("Error setting anchor range: {:?}", error);
                                        unimplemented!("Error setting anchor range: {:?}", error);
                                    }
                                }
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
                })))
                .unwrap();
        }
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
        self.window_handle = Some(window.handle());
    }

    fn dispatch_action(&mut self, action: serde_json::Value, window: &mut Window) {
        match serde_json::from_value(action) {
            Ok(WorkspaceViewAction::ToggleFileFinder) => self.toggle_file_finder(window),
            Ok(WorkspaceViewAction::ToggleDiscussion) => self.toggle_discussion(window),
            _ => eprintln!("Unrecognized action"),
        }
    }
}

impl BufferViewDelegate for WorkspaceView {
    fn set_active_buffer_view(&mut self, handle: WeakViewHandle<BufferView>) {
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
        let workspace = self.workspace.borrow();
        self.open_buffer(
            workspace.project().open_buffer(anchor.buffer_id),
            Some(anchor.range.clone()),
        );
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

    fn did_confirm(&mut self, tree_id: TreeId, path: &cross_platform::Path, _: &mut Window) {
        let workspace = self.workspace.borrow();
        self.open_buffer(workspace.project().open_path(tree_id, path), None);
    }
}

impl Stream for WorkspaceView {
    type Item = ();
    type Error = ();

    fn poll_next(&mut self, cx: &mut task::Context) -> Poll<Option<Self::Item>, Self::Error> {
        self.updates.poll_next(cx)
    }
}
