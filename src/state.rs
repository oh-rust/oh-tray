//! 运行期共享状态：当前运行中的子进程集合，以及向 UI 线程通知状态变化的通道。

use std::collections::HashMap;
use std::sync::Arc;

use command_group::AsyncGroupChild;
use tokio::sync::{Mutex, mpsc};

pub struct AppState {
    /// 运行中子进程集合，键为 `AppItem::uniq_id`。
    pub procs: Mutex<HashMap<usize, Arc<Mutex<AsyncGroupChild>>>>,
    /// 子进程状态变化时向事件循环发送 `(uniq_id, running)` 通知。
    pub tx: mpsc::UnboundedSender<(usize, bool)>,
}
