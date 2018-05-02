use futures::{unsync, Async, Future, Poll, Stream};
use never::Never;
use notify_cell::NotifyCell;
use rpc::{self, client, server};
use serde_json;
use std::cell::RefCell;
use std::rc::Rc;
use window::{View, WeakViewHandle, Window};
use workspace::{self, UserId};
use ForegroundExecutor;
use IntoShared;

pub trait DiscussionViewDelegate {
    fn anchor(&self) -> Option<workspace::Anchor>;
    fn jump(&self, anchor: &workspace::Anchor);
}

pub struct Discussion {
    messages: Vec<Message>,
    local_user_id: UserId,
    outgoing_message_txs: Vec<unsync::mpsc::UnboundedSender<Message>>,
    updates: NotifyCell<()>,
    client: Option<client::Service<DiscussionService>>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Message {
    text: String,
    anchor: Option<workspace::Anchor>,
    user_id: UserId,
}

pub struct DiscussionView<T: DiscussionViewDelegate> {
    discussion: Rc<RefCell<Discussion>>,
    updates: Box<Stream<Item = (), Error = ()>>,
    delegate: WeakViewHandle<T>,
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum DiscussionViewAction {
    Send { text: String },
    Jump { message_index: usize },
}

pub struct DiscussionService {
    remote_user_id: UserId,
    discussion: Rc<RefCell<Discussion>>,
    outgoing_messages: Box<Stream<Item = Message, Error = Never>>,
}

#[derive(Serialize, Deserialize)]
pub struct ServiceRequest {
    text: String,
    anchor: Option<workspace::Anchor>,
}

impl Discussion {
    pub fn new(local_user_id: UserId) -> Self {
        Self {
            messages: Vec::new(),
            local_user_id,
            outgoing_message_txs: Vec::new(),
            updates: NotifyCell::new(()),
            client: None,
        }
    }

    pub fn remote(
        executor: ForegroundExecutor,
        local_user_id: UserId,
        client: client::Service<DiscussionService>,
    ) -> Result<Rc<RefCell<Self>>, rpc::Error> {
        let client_updates = client.updates()?;
        let discussion = Self {
            messages: client.state()?,
            local_user_id,
            outgoing_message_txs: Vec::new(),
            updates: NotifyCell::new(()),
            client: Some(client),
        }.into_shared();

        let discussion_weak = Rc::downgrade(&discussion);
        executor
            .execute(Box::new(client_updates.for_each(move |message| {
                if let Some(discussion) = discussion_weak.upgrade() {
                    discussion.borrow_mut().push_message(message);
                }
                Ok(())
            })))
            .unwrap();
        Ok(discussion)
    }

    fn updates(&self) -> impl Stream<Item = (), Error = ()> {
        self.updates.observe()
    }

    fn outgoing_messages(&mut self) -> impl Stream<Item = Message, Error = Never> {
        let (tx, rx) = unsync::mpsc::unbounded();
        self.outgoing_message_txs.push(tx);
        rx.map_err(|_| unreachable!())
    }

    fn send(&mut self, text: String, anchor: Option<workspace::Anchor>) {
        if let Some(ref client) = self.client {
            client.request(ServiceRequest { text, anchor });
        } else {
            let user_id = self.local_user_id;
            self.push_message(Message {
                text,
                anchor,
                user_id,
            });
        }
    }

    fn push_message(&mut self, message: Message) {
        self.outgoing_message_txs
            .retain(|tx| !tx.unbounded_send(message.clone()).is_err());
        self.messages.push(message);
        self.updates.set(());
    }
}

impl<T: DiscussionViewDelegate> View for DiscussionView<T> {
    fn component_name(&self) -> &'static str {
        "Discussion"
    }

    fn render(&self) -> serde_json::Value {
        let discussion = self.discussion.borrow();
        json!({
            "messages": discussion.messages.iter().enumerate().map(|(index, message)| json!({
                "index": index,
                "text": message.text,
                "user_id": message.user_id
            })).collect::<Vec<_>>()
        })
    }

    fn dispatch_action(&mut self, action: serde_json::Value, _: &mut Window) {
        match serde_json::from_value(action) {
            Ok(DiscussionViewAction::Send { text }) => {
                if let Some(anchor) = self.delegate.map(|delegate| delegate.anchor()) {
                    self.discussion.borrow_mut().send(text, anchor);
                }
            }
            Ok(DiscussionViewAction::Jump { message_index }) => {
                let discussion = self.discussion.borrow();
                let message = &discussion.messages[message_index];
                self.delegate.map(|delegate| {
                    if let Some(ref anchor) = message.anchor {
                        delegate.jump(anchor);
                    }
                });
            }
            _ => eprintln!("Unrecognized action"),
        }
    }
}

impl<T: DiscussionViewDelegate> DiscussionView<T> {
    pub fn new(discussion: Rc<RefCell<Discussion>>, delegate: WeakViewHandle<T>) -> Self {
        let updates = discussion.borrow().updates();
        Self {
            delegate,
            discussion,
            updates: Box::new(updates),
        }
    }
}

impl<T: DiscussionViewDelegate> Stream for DiscussionView<T> {
    type Item = ();
    type Error = ();

    fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
        self.updates.poll()
    }
}

impl DiscussionService {
    pub fn new(remote_user_id: UserId, discussion: Rc<RefCell<Discussion>>) -> Self {
        let outgoing_messages = Box::new(discussion.borrow_mut().outgoing_messages());
        Self {
            remote_user_id,
            discussion,
            outgoing_messages,
        }
    }
}

impl server::Service for DiscussionService {
    type State = Vec<Message>;
    type Update = Message;
    type Request = ServiceRequest;
    type Response = ();

    fn init(&mut self, _: &rpc::server::Connection) -> Self::State {
        self.discussion.borrow().messages.clone()
    }

    fn poll_update(&mut self, _: &rpc::server::Connection) -> Async<Option<Self::Update>> {
        self.outgoing_messages.poll().unwrap()
    }

    fn request(
        &mut self,
        request: Self::Request,
        _connection: &rpc::server::Connection,
    ) -> Option<Box<Future<Item = Self::Response, Error = Never>>> {
        self.discussion.borrow_mut().push_message(Message {
            text: request.text,
            anchor: request.anchor,
            user_id: self.remote_user_id,
        });
        None
    }
}
