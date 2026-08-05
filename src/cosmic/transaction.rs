use std::env::current_exe;

use cosmic::app::Task;
use futures::{SinkExt as _, StreamExt as _};

use pakajo::events::InstallEvent;
use pakajo::install::{ChildOutcome, StreamItem, run_install_process};

use crate::detail::DetailData;

#[derive(Clone, Debug)]
pub enum TransactionMessage {
    StartInstall,
    StartRemove,
    InstallEvent(InstallEvent),
    InstallDone(ChildOutcome),
}

impl crate::PakajoApp {
    pub(crate) fn handle_transaction(
        &mut self,
        message: TransactionMessage,
    ) -> cosmic::app::Task<crate::Message> {
        match message {
            TransactionMessage::StartInstall => self.start_install(),
            TransactionMessage::StartRemove => {
                eprintln!("[pakajo] transaction: remove");
                Task::none()
            }
            TransactionMessage::InstallEvent(ev) => {
                eprintln!("[pakajo] install event: {ev:?}");
                Task::none()
            }
            TransactionMessage::InstallDone(outcome) => {
                self.transacting = false;
                match outcome {
                    ChildOutcome::Success => {
                        eprintln!("[pakajo] install succeeded");
                        // TODO: refresh updates
                        Task::none()
                    }
                    other => {
                        eprintln!("[pakajo] install outcome: {other:?}");
                        Task::none()
                    }
                }
            }
        }
    }

    fn start_install(&mut self) -> cosmic::app::Task<crate::Message> {
        if self.transacting {
            return Task::none();
        }
        let name = match &self.detail {
            DetailData::Ready { pkg, .. } => pkg.name.clone(),
            _ => return Task::none(),
        };
        let exe = match current_exe() {
            Ok(exe) => exe,
            Err(e) => {
                eprintln!("[pakajo] failed to resolve current_exe: {e}");
                return Task::none();
            }
        };
        self.transacting = true;
        let (raw_tx, mut raw_rx) = futures::channel::mpsc::channel::<StreamItem>(256);
        std::thread::spawn(move || {
            run_install_process(exe, vec![name], raw_tx, None);
        });
        Task::stream(cosmic::iced::stream::channel(
            256,
            move |mut tx: futures::channel::mpsc::Sender<cosmic::Action<crate::Message>>| async move {
                while let Some(item) = raw_rx.next().await {
                    match item {
                        StreamItem::Event(ev) => {
                            let _ = tx
                                .send(
                                    crate::Message::Transaction(TransactionMessage::InstallEvent(
                                        ev,
                                    ))
                                    .into(),
                                )
                                .await;
                        }
                        StreamItem::Done(outcome) => {
                            let _ = tx
                                .send(
                                    crate::Message::Transaction(TransactionMessage::InstallDone(
                                        outcome,
                                    ))
                                    .into(),
                                )
                                .await;
                            break;
                        }
                    }
                }
            },
        ))
    }
}
