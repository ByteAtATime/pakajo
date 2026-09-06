use cosmic::app::Task;

pub(crate) fn blocking_task<V>(
    work: impl FnOnce() -> anyhow::Result<V> + Send + 'static,
    cancelled: &'static str,
    into_message: impl FnOnce(Result<V, String>) -> cosmic::Action<crate::Message> + Send + 'static,
) -> Task<crate::Message>
where
    V: Send + 'static,
{
    let (tx, rx) = futures::channel::oneshot::channel();
    std::thread::spawn(move || {
        let _ = tx.send(work());
    });
    Task::perform(
        async move {
            match rx.await {
                Ok(Ok(value)) => Ok(value),
                Ok(Err(e)) => Err(format!("{e:#}")),
                Err(_) => Err(cancelled.to_string()),
            }
        },
        into_message,
    )
}
