use gpui::{App, AppContext, Entity, EventEmitter, Global};

use crate::app_handle::{AppEvent, UiMessage};

pub struct EventBus;

impl EventEmitter<AppEvent> for EventBus {}

struct GlobalEventBus(Entity<EventBus>);

impl Global for GlobalEventBus {}

pub fn init(messages: async_channel::Receiver<UiMessage>, cx: &mut App) {
    let bus = cx.new(|_| EventBus);
    cx.set_global(GlobalEventBus(bus.clone()));

    cx.spawn(async move |cx| {
        while let Ok(message) = messages.recv().await {
            let result = cx.update(|cx| match message {
                UiMessage::Event(event) => bus.update(cx, |_, cx| cx.emit(event)),
                UiMessage::Call(call) => call(cx),
            });
            if result.is_err() {
                break;
            }
        }
    })
    .detach();
}

pub fn event_bus(cx: &App) -> Entity<EventBus> {
    cx.global::<GlobalEventBus>().0.clone()
}
