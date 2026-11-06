//! 运行期共享状态：当前运行中的子进程集合，以及向 UI 线程通知状态变化的通道。

use std::collections::HashMap;
use std::sync::Arc;

use command_group::AsyncGroupChild;
use tokio::sync::{Mutex, mpsc};

pub struct AppState {
    /// 运行中子进程集合，键为 `AppItem::uniq_id`。
    pub procs: Mutex<HashMap<usize, Arc<Mutex<AsyncGroupChild>>>>,

    /// 记录通过 stop 菜单停止的命令
    pub stopped: Mutex<HashMap<usize, bool>>,

    /// 子进程状态变化时向事件循环发送 `(uniq_id, running)` 通知。
    pub tx: mpsc::UnboundedSender<(usize, bool)>,

    /// 唤醒 UI 事件循环。
    pub event_loop_proxy: tao::event_loop::EventLoopProxy<()>,
}

impl AppState {
    pub fn send_tx(&self, id: usize, running: bool, msg: &str) {
        let ret = self.tx.send((id, running));
        crate::log!("[app] [sendTx] {:?} ({:?},{:?}) --> {:?}", msg, id, running, ret);
        if ret.is_ok() {
            let r2 = self.event_loop_proxy.send_event(());
            crate::log!("[event_loop_proxy] {:?} ({:?},{:?}) --> {:?}", msg, id, running, r2);
        }
    }
}
