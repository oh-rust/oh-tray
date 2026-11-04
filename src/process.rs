//! 子进程生命周期管理：启动、停止、重启，以及命令构造。

use std::sync::Arc;

use command_group::AsyncCommandGroup;
use std::fs;
use tokio::process::Command;
use tokio::sync::Mutex;

use crate::config::AppItem;
use crate::state::AppState;
use crate::util::{next_runtime_id, normalize_path};

impl AppItem {
    /// 异步启动的同步包装，转发到 tokio 运行时执行。
    pub fn start(&self, state: &Arc<AppState>) {
        eprintln!("App.Start called");
        let item = self.clone();
        let state = state.clone();

        tokio::spawn(async move {
            item.a_start(&state).await;
        });
    }

    pub async fn a_start(&self, state: &Arc<AppState>) {
        eprintln!("async App.Start called");
        let item = self.clone();
        let state = state.clone();
        {
            let procs = state.procs.lock().await;
            if procs.contains_key(&item.uniq_id) {
                eprintln!("[app.a_start] process already running");
                return;
            }
        }

        item.startup(&state.clone());

        let mut cmd = build_shell(item.get_start().as_str());

        if !item.home.is_empty() {
            cmd.current_dir(item.get_home());
        }
        match cmd.group_spawn() {
            Ok(child) => {
                eprintln!(
                    "[app.a_start] start process [{}] ({}) success, pid={}",
                    item.name,
                    item.start,
                    child.id().unwrap_or(0)
                );

                let child = Arc::new(Mutex::new(child));

                state.procs.lock().await.insert(item.uniq_id, child.clone());

                state.send_tx(item.uniq_id, true, "app.a_start begin");

                let state_monitor = state.clone();
                let item_monitor = item.clone();
                let name = item_monitor.name.clone();

                tokio::spawn(async move {
                    loop {
                        let exited = {
                            let mut c = child.lock().await;
                            match c.try_wait() {
                                Ok(Some(status)) => {
                                    eprintln!(
                                        "[app.a_start] process [{}] exited with status {}",
                                        &name, status
                                    );
                                    true
                                }
                                Ok(None) => false,
                                Err(_) => true, // 出错也当作退出
                            }
                        };

                        state_monitor.send_tx(
                            item_monitor.uniq_id,
                            !exited,
                            "app.a_start [watch-loop]",
                        );

                        if exited {
                            break;
                        }
                        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                    }

                    state_monitor
                        .procs
                        .lock()
                        .await
                        .remove(&item_monitor.uniq_id);

                    state_monitor.send_tx(
                        item_monitor.uniq_id,
                        false,
                        "app.a_start loop-end send-stop",
                    );
                });
            }
            Err(e) => {
                eprintln!(
                    "[app.a_start] start process [{}] failed: {:?}",
                    item.name, e
                );
            }
        }
    }

    /// 异步停止的同步包装，转发到 tokio 运行时执行。
    pub fn stop(&self, state: &Arc<AppState>) {
        eprintln!("App.Stop called");
        let item = self.clone();
        let state = state.clone();

        tokio::spawn(async move {
            item.a_stop(&state).await;
        });
    }

    pub async fn a_stop(&self, state: &Arc<AppState>) {
        eprintln!("async App.Stop called");
        let item = self.clone();
        let state = state.clone();

        // 先尝试使用自定义的 stop 命令停止应用
        if !item.stop.is_empty() {
            let p = normalize_path(&item.stop);
            let mut cmd = build_shell(p.as_str());

            if !item.home.is_empty() {
                cmd.current_dir(item.get_home());
            }
            match cmd.group_spawn() {
                Ok(mut child) => {
                    let ret = child.wait().await;
                    eprintln!(
                        "[app.a_stop] {} execute stop command success, status={:?}",
                        &item.name, ret
                    );
                }
                Err(e) => {
                    eprintln!(
                        "[app.a_stop] {} execute stop command failed:{:?}",
                        &item.name, e
                    );
                }
            }
        }

        let maybe_child = {
            let mut procs = state.procs.lock().await;
            procs.remove(&item.uniq_id)
        };
        if let Some(child) = maybe_child {
            let mut c = child.lock().await;
            let pid = c.id().unwrap_or(0);
            let ret1 = c.kill().await;
            let ret2 = c.wait().await;
            eprintln!(
                "[app.a_stop] stop process {}, kill={:?}, wait={:?}",
                pid, ret1, ret2
            );
        } else {
            eprintln!("[app.a_stop] process not found");
        }

        state.send_tx(item.uniq_id, false, "app.a_stop");
    }

    pub fn restart(&self, state: &Arc<AppState>) {
        eprintln!("App.Restart called");
        let item = self.clone();
        let state = state.clone();
        tokio::spawn(async move {
            eprintln!("restart >>>>>>>>>>>>>>>>>>>>>>>>>>>>>");
            item.a_stop(&state).await;
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            item.a_start(&state).await;
            eprintln!("restart <<<<<<<<<<<<<<<<<<<<<<<<<<<<<");
        });
    }

    pub fn open_browser(&self, _state: &Arc<AppState>) {
        eprintln!("{} open browser ...", &self.address);
        if let Err(e) = webbrowser::open(&self.address) {
            eprintln!("Failed to open browser for {}: {:?}", self.address, e);
        }
    }

    fn startup(&self, _state: &Arc<AppState>) {
        eprintln!("{} startup ...", &self.name);
        let startup_commands = self.startup_commands.clone();
        let item = self.clone();
        tokio::spawn(async move {
            for command in &startup_commands {
                execute_command(item.clone(), command).await;
            }
        });

        for p in &self.startup_cleanup {
            remove_path(self.clone(), p);
        }

        eprintln!("{} startup done", &self.name);
    }
}

/// 根据平台构造用于启动子命令的 shell 命令。
///
/// Windows 下通过临时 vbs 脚本隐藏控制台窗口启动；其他平台使用 `sh -c`。
fn build_shell(cmd: &str) -> Command {
    #[cfg(target_os = "windows")]
    {
        use std::env;
        use std::fs;
        use std::os::windows::process::CommandExt;

        let temp_dir = env::temp_dir();
        let file_name = format!("oh_tray_{}.vbs", next_runtime_id());
        let vbs_path = temp_dir.join(file_name);

        let vbs_content = format!(
            "CreateObject(\"Wscript.Shell\").Run \"cmd /C {}\", 0, True",
            cmd.replace("\"", "\"\"") // 处理引号转义
        );

        let _ = fs::write(&vbs_path, vbs_content);
        let mut c = Command::new("wscript.exe");

        c.arg(&vbs_path);
        c.as_std_mut().creation_flags(0x08000000);

        c
    }

    #[cfg(not(target_os = "windows"))]
    {
        let mut c = Command::new("sh");
        c.args(["-c", cmd]);
        c
    }
}

/// Execute a startup command.
async fn execute_command(item: AppItem, command: &str) {
    let command = command.trim();

    if command.is_empty() {
        return;
    }
    let mut c = build_shell(command);
    if !item.home.is_empty() {
        c.current_dir(item.get_home());
    }

    let output = build_shell(command).output().await;
    match output {
        Ok(output) => {
            if output.status.success() {
                println!("[STARTUP] Command executed: {}", command);
            } else {
                eprintln!("[STARTUP] Command failed ({}): {}", output.status, command);

                let stderr = String::from_utf8_lossy(&output.stderr);
                if !stderr.trim().is_empty() {
                    eprintln!("{}", stderr.trim());
                }
            }
        }
        Err(err) => {
            eprintln!("[STARTUP] Failed to execute command '{}': {}", command, err);
        }
    }
}

/// Remove a file or directory.
fn remove_path(item: AppItem, path: &str) {
    let mut path = std::path::PathBuf::from(normalize_path(path));

    if path.is_relative() && !item.home.is_empty() {
        path = std::path::Path::new(&item.home).join(path);
    }

    match fs::symlink_metadata(&path) {
        Ok(metadata) => {
            let result = if metadata.file_type().is_symlink() || metadata.is_file() {
                fs::remove_file(&path)
            } else if metadata.is_dir() {
                fs::remove_dir_all(&path)
            } else {
                eprintln!("[CLEANUP] Unsupported file type: {}", path.display());
                return;
            };

            match result {
                Ok(()) => {
                    println!("[CLEANUP] Removed: {}", path.display());
                }
                Err(err) => {
                    eprintln!("[CLEANUP] Failed to remove '{}': {}", path.display(), err);
                }
            }
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            println!("[CLEANUP] Not found: {}", path.display());
        }
        Err(err) => {
            eprintln!("[CLEANUP] Failed to access '{}': {}", path.display(), err);
        }
    }
}
