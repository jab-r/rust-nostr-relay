use crate::{
    message::{ClientMessage, OutgoingMessage, ReadEvent, Subscription},
    setting::SettingWrapper,
    Session,
};
use actix_web::web::ServiceConfig;
use nostr_db::Event;

pub enum ExtensionMessageResult {
    /// Continue run the next extension message method, the server takes over finally.
    Continue(ClientMessage),
    /// Stop run the next, send outgoing message to client.
    Stop(OutgoingMessage),
    /// Stop run the next, does not send any messages to the client.
    Ignore,
}

impl From<OutgoingMessage> for ExtensionMessageResult {
    fn from(value: OutgoingMessage) -> Self {
        Self::Stop(value)
    }
}

/// Result of processing a REQ message
pub enum ExtensionReqResult {
    /// Continue with normal database query
    Continue,
    /// Add events to the response (will still do database query)
    AddEvents(Vec<Event>),
    /// Completely handle the request (skip database query)
    Handle(Vec<Event>),
}

/// Result of post-processing query results
pub struct PostProcessResult {
    /// Events to return to client (may be filtered/modified from original)
    pub events: Vec<Event>,
    /// Events that were consumed (for tracking purposes)
    pub consumed_events: Vec<Event>,
}

/// Extension for user session
pub trait Extension: Send + Sync {
    fn name(&self) -> &'static str;

    /// Execute when added to extension list and setting reload
    #[allow(unused_variables)]
    fn setting(&mut self, setting: &SettingWrapper) {}

    /// config actix web service
    #[allow(unused_variables)]
    fn config_web(&mut self, cfg: &mut ServiceConfig) {}

    /// Execute after a user connect
    #[allow(unused_variables)]
    fn connected(&self, session: &mut Session, ctx: &mut <Session as actix::Actor>::Context) {}

    /// Execute when connection lost
    #[allow(unused_variables)]
    fn disconnected(&self, session: &mut Session, ctx: &mut <Session as actix::Actor>::Context) {}

    /// Execute when message incoming
    #[allow(unused_variables)]
    fn message(
        &self,
        msg: ClientMessage,
        session: &mut Session,
        ctx: &mut <Session as actix::Actor>::Context,
    ) -> ExtensionMessageResult {
        ExtensionMessageResult::Continue(msg)
    }

    /// Intercept REQ messages before database query
    #[allow(unused_variables)]
    fn process_req(&self, session_id: usize, subscription: &Subscription) -> ExtensionReqResult {
        ExtensionReqResult::Continue
    }

    /// Post-process query results before sending to client
    #[allow(unused_variables)]
    fn post_process_query_results(
        &self,
        session_id: usize,
        subscription: &Subscription,
        events: Vec<Event>,
    ) -> PostProcessResult {
        PostProcessResult {
            events,
            consumed_events: vec![],
        }
    }
}

/// extensions
#[derive(Default)]
pub struct Extensions {
    list: Vec<Box<dyn Extension>>,
}

impl Extensions {
    pub fn add<E: Extension + 'static>(&mut self, ext: E) {
        self.list.push(Box::new(ext));
    }

    pub fn call_setting(&mut self, setting: &SettingWrapper) {
        for ext in &mut self.list {
            ext.setting(setting);
        }
    }

    pub fn call_config_web(&mut self, cfg: &mut ServiceConfig) {
        for ext in &mut self.list {
            ext.config_web(cfg);
        }
    }

    pub fn call_connected(
        &self,
        session: &mut Session,
        ctx: &mut <Session as actix::Actor>::Context,
    ) {
        for ext in &self.list {
            ext.connected(session, ctx);
        }
    }

    pub fn call_disconnected(
        &self,
        session: &mut Session,
        ctx: &mut <Session as actix::Actor>::Context,
    ) {
        for ext in &self.list {
            ext.disconnected(session, ctx);
        }
    }

    pub fn call_message(
        &self,
        msg: ClientMessage,
        session: &mut Session,
        ctx: &mut <Session as actix::Actor>::Context,
    ) -> ExtensionMessageResult {
        let mut msg = msg;
        for ext in &self.list {
            match ext.message(msg, session, ctx) {
                ExtensionMessageResult::Continue(m) => {
                    msg = m;
                }
                ExtensionMessageResult::Stop(o) => {
                    return ExtensionMessageResult::Stop(o);
                }
                ExtensionMessageResult::Ignore => {
                    return ExtensionMessageResult::Ignore;
                }
            };
        }
        ExtensionMessageResult::Continue(msg)
    }

    pub fn call_process_req(
        &self,
        session_id: usize,
        subscription: &Subscription,
    ) -> (ExtensionReqResult, Vec<Event>) {
        let mut additional_events = Vec::new();
        
        for ext in &self.list {
            match ext.process_req(session_id, subscription) {
                ExtensionReqResult::Continue => continue,
                ExtensionReqResult::AddEvents(mut events) => {
                    additional_events.append(&mut events);
                }
                ExtensionReqResult::Handle(events) => {
                    return (ExtensionReqResult::Handle(events), vec![]);
                }
            }
        }
        
        if !additional_events.is_empty() {
            (ExtensionReqResult::AddEvents(additional_events.clone()), additional_events)
        } else {
            (ExtensionReqResult::Continue, vec![])
        }
    }

    pub fn call_post_process_query_results(
        &self,
        session_id: usize,
        subscription: &Subscription,
        mut events: Vec<Event>,
    ) -> PostProcessResult {
        let mut all_consumed_events = Vec::new();
        
        for ext in &self.list {
            let result = ext.post_process_query_results(session_id, subscription, events);
            events = result.events;
            all_consumed_events.extend(result.consumed_events);
        }
        
        PostProcessResult {
            events,
            consumed_events: all_consumed_events,
        }
    }
}
