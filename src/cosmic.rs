use cosmic::widget::container;
use cosmic::{
    Application, Element,
    app::{self, Core, Settings, Task},
    executor,
};

fn main() -> cosmic::iced::Result {
    let settings = Settings::default();
    let flags = ();
    app::run::<PakajoApp>(settings, flags)
}

pub struct PakajoApp {
    core: Core,
}

impl Application for PakajoApp {
    type Executor = executor::Default;
    type Flags = ();
    type Message = Message;
    const APP_ID: &'static str = "com.pakajo.Pakajo";

    fn core(&self) -> &Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut Core {
        &mut self.core
    }

    fn init(core: Core, _flags: Self::Flags) -> (Self, Task<Self::Message>) {
        (PakajoApp { core }, Task::none())
    }

    fn update(&mut self, _message: Self::Message) -> Task<Self::Message> {
        Task::none()
    }

    fn view(&self) -> Element<'_, Self::Message> {
        container(cosmic::widget::text::heading("pakajo")).into()
    }
}

#[derive(Clone, Debug)]
pub enum Message {}
