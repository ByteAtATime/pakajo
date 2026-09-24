use cosmic::app::Task;

use crate::components::transaction::Transaction;

#[derive(Clone, Debug)]
pub enum SysupgradeMessage {
    Start,
}

impl crate::PakajoApp {
    pub(crate) fn handle_sysupgrade(&mut self, message: SysupgradeMessage) -> Task<crate::Message> {
        match message {
            SysupgradeMessage::Start => {
                if self.transaction.as_ref().is_some_and(|t| t.is_active()) {
                    return Task::none();
                }
                let (transaction, task) = Transaction::start_sysupgrade();
                self.transaction = Some(transaction);
                task
            }
        }
    }
}
