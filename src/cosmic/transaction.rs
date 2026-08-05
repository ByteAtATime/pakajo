use cosmic::app::Task;

#[derive(Clone, Debug)]
pub enum TransactionMessage {
    StartInstall,
    StartRemove,
}

impl crate::PakajoApp {
    pub(crate) fn handle_transaction(
        &mut self,
        message: TransactionMessage,
    ) -> cosmic::app::Task<crate::Message> {
        match message {
            TransactionMessage::StartInstall => {
                eprintln!("[pakajo-cosmic] transaction: install (wired in phase 4)");
                Task::none()
            }
            TransactionMessage::StartRemove => {
                eprintln!("[pakajo-cosmic] transaction: remove (wired in phase 4)");
                Task::none()
            }
        }
    }
}
