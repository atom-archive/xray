extern crate xray_core;

use futures::{Future, Stream, Sink};
use futures::sync::mpsc;
use messages::{IncomingMessage, OutgoingMessage};
use std::cell::RefCell;
use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::rc::Rc;
use serde_json;
use workspace::WorkspaceView;

type OutboundSender = mpsc::UnboundedSender<OutgoingMessage>;

pub struct App {
    shared: Rc<RefCell<Shared>>,
}

struct Shared {
    next_workspace_id: usize,
    application_sender: Option<OutboundSender>,
    window_senders: HashMap<usize, OutboundSender>,
    workspace_views: HashMap<usize, WorkspaceView>,
}

enum Client {
    Unknown,
    Application,
    Window{workspace_id: usize},
}

impl App {
    pub fn new() -> Self {
        Self {
            shared: Rc::new(RefCell::new(Shared {
                next_workspace_id: 1,
                application_sender: None,
                window_senders: HashMap::new(),
                workspace_views: HashMap::new(),
            })),
        }
    }

    pub fn add_connection<'a, S>(&mut self, socket: S) -> Box<'a + Future<Item = (), Error = ()>>
    where
        S: 'a + Stream<Item = IncomingMessage, Error = io::Error> + Sink<SinkItem = OutgoingMessage>,
    {
        let (outgoing, incoming) = socket.split();
        let (tx, rx) = mpsc::unbounded();

        let shared = self.shared.clone();
        let mut client = Client::Unknown;
        let handle_incoming = incoming.for_each(move |message| {
            shared.borrow_mut().handle_message(&tx, message, &mut client);
            Ok(())
        });

        let send_outgoing = outgoing.send_all(rx.map_err(|_| unreachable!())).then(|_| Ok(()));

        Box::new(handle_incoming.select(send_outgoing).then(|_| Ok(())))
    }
}

impl Shared {
    fn handle_message(&mut self, sender: &OutboundSender, message: IncomingMessage, client: &mut Client) {
        match client {
            &mut Client::Unknown => {
                match message {
                    IncomingMessage::StartApplication => {
                        self.start_application(sender);
                        *client = Client::Application;
                    }
                    IncomingMessage::StartWindow { workspace_id } => {
                        self.start_window(sender, workspace_id);
                        *client = Client::Window{workspace_id};
                    }
                    IncomingMessage::OpenWorkspace { paths } => {
                        self.open_workspace(sender, paths);
                    },
                    _ => panic!("Unexpected message"),
                }
            },
            &mut Client::Application => {
            },
            &mut Client::Window{ workspace_id } => {
                match message {
                    IncomingMessage::Action{view_type, view_id, action} => {
                        self.handle_action(
                            sender,
                            workspace_id,
                            view_type,
                            view_id,
                            action,
                        )
                    }
                    _ => panic!("Unexpected message")
                }
            }
        }
    }

    fn start_application(&mut self, sender: &OutboundSender) {
        self.application_sender = Some(sender.clone());
        sender.unbounded_send(OutgoingMessage::Acknowledge);
    }

    fn start_window(&mut self, sender: &OutboundSender, workspace_id: usize) {
        self.window_senders.insert(workspace_id, sender.clone());
        sender.unbounded_send(OutgoingMessage::WindowState{});
    }

    fn open_workspace(&mut self, sender: &OutboundSender, paths: Vec<PathBuf>) {
        let workspace_id = self.next_workspace_id;
        self.next_workspace_id += 1;
        self.workspace_views.insert(workspace_id, WorkspaceView::new());
        if let Some(ref mut sender) = self.application_sender {
            sender.unbounded_send(OutgoingMessage::OpenWindow{ workspace_id });
        }
        sender.unbounded_send(OutgoingMessage::Acknowledge);
    }

    fn handle_action(
        &mut self,
        sender: &OutboundSender,
        workspace_id: usize,
        view_type: String,
        view_id: usize,
        action: serde_json::Value,
    ) {
        if let &mut Some(ref mut workspace_view) = &mut self.workspace_views.get_mut(&workspace_id) {
            workspace_view.handle_action(view_type, view_id, action);
        }
    }
}
