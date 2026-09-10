use std::io::{self, BufReader};
use std::process::{Child, ExitStatus};

use futures::SinkExt as _;
use futures::channel::mpsc;

use crate::events::{InstallEvent, InstallSink, read_event_stream};

#[derive(Clone, Debug)]
pub enum ChildOutcome {
    Success,
    Dismissed,
    NotFound,
    Failed(String),
}

pub enum StreamItem {
    Event(InstallEvent),
    Done(ChildOutcome),
}

pub fn map_outcome(status: io::Result<ExitStatus>) -> ChildOutcome {
    let code = match status {
        Err(error) => return ChildOutcome::Failed(error.to_string()),
        Ok(status) => status.code(),
    };
    match code {
        Some(0) => ChildOutcome::Success,
        Some(126) => ChildOutcome::Dismissed,
        Some(127) => ChildOutcome::NotFound,
        Some(exit) => ChildOutcome::Failed(format!("install failed (exit {exit})")),
        None => ChildOutcome::Failed("install killed by signal".to_string()),
    }
}

pub fn send_item(tx: &mut mpsc::Sender<StreamItem>, item: StreamItem) {
    futures::executor::block_on(tx.send(item)).ok();
}

pub struct ChannelSink {
    tx: mpsc::Sender<StreamItem>,
}

impl ChannelSink {
    pub fn new(tx: mpsc::Sender<StreamItem>) -> Self {
        Self { tx }
    }
}

impl InstallSink for ChannelSink {
    fn event(&mut self, event: InstallEvent) {
        send_item(&mut self.tx, StreamItem::Event(event));
    }
}

pub fn stream_child<S: InstallSink + ?Sized>(
    mut child: Child,
    sink: &mut S,
) -> std::io::Result<ExitStatus> {
    let stdout = child.stdout.take().expect("piped stdout");
    read_event_stream(BufReader::new(stdout), sink);
    child.wait()
}
