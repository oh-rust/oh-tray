//! 配置加载与配置数据结构。

use std::sync::{Arc, Once};

use anyhow::Result;
use clap::Parser;
use serde::{Deserialize, Serialize};

use crate::icon::{load_default, load_from_path};
use crate::state::AppState;
use crate::util::{default_true, normalize_path, redirect_stderr};

#[derive(Parser, Debug)]
struct Args {
    #[arg(short, long, default_value = "conf/tray.yml")]
    pub config: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AppConfig {
    #[serde(default)]
    pub title: String,

    /// tray 的根目录，默认为空
    /// 若不为空，则启动应用后先切换到此目录下
    #[serde(default)]
    pub home: String,

    /// 应用图标的地址
    #[serde(default)]
    pub icon: String,

    /// 日志文件地址前缀，默认为当前
    #[serde(default)]
    pub log_prefix: String,

    #[serde(default)]
    pub groups: Vec<AppItem>,
}

/// 一个应用，也是一条菜单的配置信息
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AppItem {
    /// 标题
    pub name: String,

    /// 当前条目的根目录，启动 tray 后,启动此 app 时，使用此为 HOME 目录
    #[serde(default)]
    pub home: String,

    /// 启动命令，必填，如 “./bin/aimux.exe”，“docker compose up”
    pub start: String,

    #[serde(skip, default = "default_startup_once")]
    pub startup_once: Arc<Once>,

    /// tray 启动前执行的命令列表
    #[serde(default)]
    pub startup_commands: Vec<String>,

    /// tray 启动时需要删除的文件列表
    #[serde(default)]
    pub startup_cleanup: Vec<String>,

    /// 停止命令，可选
    #[serde(default)]
    pub stop: String,

    /// 是否自动启动
    #[serde(default = "default_true")]
    pub auto_start: bool,

    /// 是否隐藏菜单项目
    #[serde(default)]
    pub hidden: bool,

    /// 浏览器访问地址
    #[serde(default)]
    pub address: String,

    /// 是否自动在浏览器打开
    #[serde(default)]
    pub auto_open: bool,

    /// 是否启用进程保活，进程退出后自动重启
    #[serde(default)]
    pub keep_alive: bool,

    /// 最大存活时长（秒），0 表示不限制
    #[serde(default)]
    pub max_lifetime: u64,

    #[serde(skip_deserializing, default = "crate::util::next_runtime_id")]
    pub uniq_id: usize,

    #[serde(default)]
    pub opens: Vec<AppOpenURI>,
}

fn default_startup_once() -> Arc<Once> {
    Arc::new(Once::new())
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AppOpenURI {
    pub title: String,
    pub uri: String,
}

impl AppOpenURI {
    pub fn open(&self) {
        crate::log!("{} open browser {} ...", &self.title, &self.uri);
        if let Err(e) = webbrowser::open(&self.uri) {
            crate::log!("Failed to open browser for {}: {:?}", self.uri, e);
        }
    }
}

impl AppConfig {
    pub fn get_home(&self) -> String {
        normalize_path(&self.home)
    }
    pub fn get_icon(&self) -> String {
        normalize_path(&self.icon)
    }
    pub fn get_log_prefix(&self) -> String {
        normalize_path(&self.log_prefix)
    }

    pub fn load_icon(&self) -> tray_icon::Icon {
        if self.icon.is_empty() {
            return load_default();
        }
        load_from_path(&self.get_icon())
    }

    pub fn setup(&self) {
        if !self.home.is_empty() {
            std::env::set_current_dir(self.get_home()).unwrap();
        }

        if !self.log_prefix.is_empty() && self.log_prefix.ne("no") {
            let filename = format!("{}_stderr.log", self.get_log_prefix());
            match std::fs::File::create(&filename) {
                Ok(file) => {
                    redirect_stderr(&file);

                    // 避免 file 被 drop
                    std::mem::forget(file);
                }
                Err(e) => {
                    crate::log!("create log file {}: {}", filename, e);
                }
            }
        }
    }

    pub fn auto_start_all(&self, state: &Arc<AppState>) {
        for item in &self.groups {
            if item.auto_start {
                item.menu_restart(state);
                let _ = state.tx.send((item.uniq_id, true));
                if item.auto_open {
                    item.open_browser(state);
                }
            } else {
                // 确保服务类的进程，在 tray 启动的时候，是停止状态
                // 如用 docker compose 启动的进程，在关机后，下次会自动启动
                item.menu_stop(state);
            }
        }
    }

    pub fn stop_all(&self, state: &Arc<AppState>) {
        crate::log!("stopping all apps ...");
        let groups = self.groups.clone();
        let state = state.clone();

        let (tx, rx) = std::sync::mpsc::channel();
        tokio::spawn(async move {
            let futures = groups.iter().map(|child| {
                let child = child.clone();
                let cs = state.clone();
                async move {
                    child.a_stop(&cs).await;
                }
            });
            crate::log!("async {} futures", futures.len());
            futures::future::join_all(futures).await;
            let _ = tx.send(());
        });
        let _ = rx.recv();
        crate::log!("stopped all apps finished");
    }
}

impl AppItem {
    pub fn get_home(&self) -> String {
        normalize_path(&self.home)
    }
    pub fn get_start(&self) -> String {
        normalize_path(&self.start)
    }
}

/// 从命令行指定的配置文件加载并反序列化 `AppConfig`。
pub fn load_cfg() -> Result<AppConfig> {
    let args = Args::parse();

    let cfg = config::Config::builder().add_source(config::File::with_name(&args.config)).build()?;

    Ok(cfg.try_deserialize()?)
}
