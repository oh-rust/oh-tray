#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod config;
mod i18n;
mod icon;
mod process;
mod state;
mod tray;
mod util;

use std::collections::HashMap;
use std::sync::Arc;
use tao::event::Event;
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tokio::sync::{Mutex, mpsc};
use tray_icon::menu::MenuEvent;

use crate::config::load_cfg;
use crate::state::AppState;
use crate::tray::{EVENT_QUIT, build_tray};

#[tokio::main]
async fn main() {
    let cfg = load_cfg().unwrap();
    cfg.setup();
    log!("-------------------boot-----------------------");

    let event_loop = EventLoopBuilder::new().build();

    let (tx, mut rx) = mpsc::unbounded_channel();

    let state = Arc::new(AppState {
        procs: Mutex::new(HashMap::new()),
        tx,
        event_loop_proxy: event_loop.create_proxy(),
        stopped: Mutex::new(HashMap::new()),
    });

    // 构建托盘图标与菜单；tray 字段必须保持存活
    let mut tray_ui = build_tray(&cfg, &state);

    {
        let cfg1 = cfg.clone();
        let state1 = state.clone();
        tokio::task::spawn_blocking(move || {
            cfg1.auto_start_all(&state1);
        });
    }

    let state_destroy = state.clone();
    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        match event {
            Event::LoopDestroyed => {
                log!("receive Event::LoopDestroyed -> tray exiting");
                cfg.stop_all(&state_destroy);
            }
            Event::UserEvent(()) => {
                // EventLoopProxy 唤醒事件循环
                log!(" receive Event::UserEvent");
            }
            Event::MainEventsCleared => {
                while let Ok((id, running)) = rx.try_recv() {
                    log!("receive tx ({},{})", id, running);
                    if let Some(ui) = tray_ui.ui_map.get_mut(&id) {
                        ui.set_running(running);
                    }
                }

                if let Ok(event) = MenuEvent::receiver().try_recv() {
                    let eid = event.id.0.as_str();
                    log!("receive event {:?} , eid={})", event, eid);
                    if let Some(action) = tray_ui.actions.get_mut(eid) {
                        log!("action executed");
                        action();
                    }

                    if eid == EVENT_QUIT {
                        *control_flow = ControlFlow::Exit;
                        log!("event EVENT_QUIT");
                        return;
                    }
                }
            }
            _ => {}
        }
    });
}
