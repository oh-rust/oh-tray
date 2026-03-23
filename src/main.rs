#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use clap::Parser;
use command_group::{AsyncCommandGroup, AsyncGroupChild};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tao::event_loop::{ControlFlow, EventLoopBuilder};

use tokio::process::Command;
use tokio::sync::{Mutex, mpsc};

use tray_icon::{
    TrayIconBuilder,
    menu::{IconMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu},
};

#[derive(Parser, Debug)]
struct Args {
    #[arg(short, long, default_value = "conf/tray.yaml")]
    pub config: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct AppConfig {
    #[serde(default)]
    title: String,

    #[serde(default)]
    icon: String,

    #[serde(default)]
    groups: Vec<AppItem>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct AppItem {
    name: String,

    #[serde(default)]
    home: String,

    start: String,

    #[serde(default)]
    stop: String,

    #[serde(default)]
    restart: String,

    #[serde(default = "default_true")]
    auto_start: bool,

    #[serde(default)]
    address: String, // 浏览器访问地址

    #[serde(default)]
    auto_open: bool, // 是否自动在浏览器打开

    #[serde(skip_deserializing, default = "next_runtime_id")]
    uniq_id: usize,
}

fn default_true() -> bool {
    true
}

static NEXT_ID: AtomicUsize = AtomicUsize::new(1);
fn next_runtime_id() -> usize {
    NEXT_ID.fetch_add(1, Ordering::SeqCst)
}

impl AppConfig {
    fn load_icon(&self) -> tray_icon::Icon {
        if self.icon.is_empty() {
            return load_icon();
        }
        if let Ok(reader) = image::ImageReader::open(self.icon.as_str()) {
            if let Ok(img) = reader.decode() {
                let rgba = img.to_rgba8(); // 转 RGBA
                let (width, height) = rgba.dimensions();

                if let Ok(icon) = tray_icon::Icon::from_rgba(
                    rgba.into_raw(), // Vec<u8>
                    width,
                    height,
                ) {
                    return icon;
                }
            }
        }

        load_icon()
    }
}

// ================= 状态 =================

struct AppState {
    procs: Mutex<HashMap<usize, Arc<Mutex<AsyncGroupChild>>>>,
    tx: mpsc::UnboundedSender<(usize, bool)>,
}

impl AppState {
    fn stop_all(self: &Arc<Self>) {
        eprintln!("stopping all apps ...");
        let state = self.clone();
        tokio::spawn(async move {
            let procs = state.procs.lock().await;

            for (index, child) in procs.iter() {
                let mut c = child.lock().await;
                let ret1 = c.kill().await;
                let ret2 = c.wait().await;
                eprintln!("kill {} {:?},{:?}", index, ret1, ret2);
            }
        });
    }
}

// ================= UI =================

struct UiEntry {
    start: MenuItem,
    stop: MenuItem,
    restart: MenuItem,
    title: TitleMenu,
    open: Option<IconMenuItem>,
}

enum TitleMenu {
    Submenu(Submenu),
    // Icon(IconMenuItem),
}

impl UiEntry {
    fn set_running(&self, running: bool) {
        self.start.set_enabled(!running);
        self.stop.set_enabled(running);
        self.restart.set_enabled(running);

        match &self.title {
            TitleMenu::Submenu(title_menu) => {
                title_menu.set_icon(Some(get_menu_icon(running)));
            } // TitleMenu::Icon(title_menu) => {
              //     title_menu.set_icon(Some(get_menu_icon(running)));
              // }
        }

        if let Some(icon) = &self.open {
            icon.set_enabled(running);
        }
    }
}

// ================= 进程 =================
// use std::process::Stdio;

impl AppItem {
    fn start(&self, state: &Arc<AppState>) {
        eprintln!("App.Start called");
        let item = self.clone();
        let state = state.clone();

        tokio::spawn(async move {
            item.a_start(&state).await;
        });
    }

    async fn a_start(&self, state: &Arc<AppState>) {
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

        let mut cmd = build_shell(&item.start);

        if !item.home.is_empty() {
            cmd.current_dir(&item.home);
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
                let ret = state.tx.send((item.uniq_id, true));
                eprintln!("[app.a_start] tx.send running: {:?}", ret);

                let state_monitor = state.clone();
                let item_monitor = item.clone();
                let name = item_monitor.name.clone();

                tokio::spawn(async move {
                    loop {
                        let exited = {
                            let mut c = child.lock().await;
                            match c.try_wait() {
                                Ok(Some(status)) => {
                                    eprintln!("[app.a_start] process [{}] exited with status {}", &name, status);
                                    true
                                }
                                Ok(None) => false,
                                Err(_) => true, // 出错也当作退出
                            }
                        };

                        if exited {
                            break;
                        }
                        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                    }

                    state_monitor.procs.lock().await.remove(&item_monitor.uniq_id);

                    let ret = state_monitor.tx.send((item_monitor.uniq_id, false));
                    eprintln!("[app.a_start] [watch] tx.send stop {:?}", ret);
                });
            }
            Err(e) => {
                eprintln!("[app.a_start] start process [{}] failed: {:?}", item.name, e);
            }
        }
    }

    fn stop(&self, state: &Arc<AppState>) {
        eprintln!("App.Stop called");
        let item = self.clone();
        let state = state.clone();

        tokio::spawn(async move {
            item.a_stop(&state).await;
        });
    }

    async fn a_stop(&self, state: &Arc<AppState>) {
        eprintln!("async App.Stop called");
        let item = self.clone();
        let state = state.clone();
        let maybe_child = {
            let mut procs = state.procs.lock().await;
            procs.remove(&item.uniq_id)
        };
        if let Some(child) = maybe_child {
            let mut c = child.lock().await;
            let pid = c.id().unwrap_or(0);
            let ret1 = c.kill().await;
            let ret2 = c.wait().await;
            eprintln!("[app.a_stop] stop process {}, kill={:?}, wait={:?}", pid, ret1, ret2);
        } else {
            eprintln!("[app.a_stop] process not found");
        }

        let ret = state.tx.send((item.uniq_id, false));
        eprintln!("[app.a_stop]tx.send stop {:?}", ret);
    }

    fn restart(&self, state: &Arc<AppState>) {
        eprintln!("App.Restart called");
        let item = self.clone();
        let state = state.clone();
        tokio::spawn(async move {
            eprintln!("restart >>>>>>>>>>>>>>>>>>>>>>>>>>>>>");
            item.a_stop(&state).await;
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            item.a_start(&state).await;
            eprintln!("restart <<<<<<<<<<<<<<<<<<<<<<<<<<<<<");
        });
    }

    fn open_browser(&self, _state: &Arc<AppState>) {
        eprintln!("{} open browser ...", &self.address);
        if let Err(e) = webbrowser::open(&self.address) {
            eprintln!("Failed to open browser for {}: {:?}", self.address, e);
        }
    }
}

// ================= util =================

// #[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

use std::env;
use std::fs;

fn build_shell(cmd: &str) -> Command {
    if cfg!(target_os = "windows") {
        // let has_ps = std::process::Command::new("powershell")
        //     .arg("-Command")
        //     .arg("exit")
        //     .stdout(Stdio::null())
        //     .stderr(Stdio::null())
        //     .status()
        //     .map(|s| s.success())
        //     .unwrap_or(false);
        //
        // let mut c;
        // if has_ps {
        //     c = Command::new("powershell");
        //     // -WindowStyle Hidden 是双重保险
        //     c.args(["-NoProfile", "-WindowStyle", "Hidden", "-Command", cmd]);
        // } else {
        //     c = Command::new("cmd");
        //     c.args(["/C", cmd]);
        // }
        // c.as_std_mut().creation_flags(0x08000000);
        // c
        let temp_dir = env::temp_dir();
        let unique_id = Box::into_raw(Box::new(0)) as usize;
        let file_name = format!("oh_tray_{}.vbs", unique_id);
        let vbs_path = temp_dir.join(file_name);

        let vbs_content = format!(
            "CreateObject(\"Wscript.Shell\").Run \"cmd /C {}\", 0, True",
            cmd.replace("\"", "\"\"") // 处理引号转义
        );

        let _ = fs::write(&vbs_path, vbs_content);
        let mut c = Command::new("wscript.exe");
        c.arg(&vbs_path);
        c.as_std_mut().creation_flags(0x08000000);

        tokio::spawn(async move {
            tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;
            let _ = std::fs::remove_file(vbs_path);
        });

        c
    } else {
        let mut c = Command::new("sh");
        c.args(["-c", cmd]);
        c
    }
}

// ================= 图标 =================

fn circle_rgba(r: u8, g: u8, b: u8) -> Vec<u8> {
    let mut pixels = vec![0u8; 32 * 32 * 4];
    for y in 0..32 {
        for x in 0..32 {
            let dx = x as f32 - 16.0;
            let dy = y as f32 - 16.0;
            let dist = (dx * dx + dy * dy).sqrt();

            let idx = (y * 32 + x) * 4;

            if dist < 14.0 {
                pixels[idx] = r;
                pixels[idx + 1] = g;
                pixels[idx + 2] = b;
                pixels[idx + 3] = 255;
            }
        }
    }
    pixels
}

fn get_menu_icon(running: bool) -> tray_icon::menu::Icon {
    let rgba = if running {
        circle_rgba(0, 200, 0)
    } else {
        circle_rgba(200, 0, 0)
    };

    tray_icon::menu::Icon::from_rgba(rgba, 32, 32).unwrap()
}

fn load_icon() -> tray_icon::Icon {
    let img = image::load_from_memory(include_bytes!("../icons/tray.png"))
        .unwrap()
        .into_rgba8();

    let (w, h) = img.dimensions();
    tray_icon::Icon::from_rgba(img.into_raw(), w, h).unwrap()
}

use std::sync::OnceLock;

static IS_ZH: OnceLock<bool> = OnceLock::new();

fn is_zh() -> bool {
    *IS_ZH.get_or_init(|| {
        sys_locale::get_locale()
            .unwrap_or_else(|| "en-US".to_string())
            .to_lowercase()
            .starts_with("zh")
    })
}

fn lang_text<'a>(zh: &'a str, en: &'a str) -> &'a str {
    if is_zh() { zh } else { en }
}

// ================= main =================

const EVENT_QUIT: &str = "quit";

#[tokio::main]
async fn main() {
    let cfg = load_cfg().unwrap();

    let event_loop = EventLoopBuilder::new().build();

    let (tx, mut rx) = mpsc::unbounded_channel();

    let state = Arc::new(AppState {
        procs: Mutex::new(HashMap::new()),
        tx,
    });

    let menu = Menu::new();
    let mut actions: HashMap<String, Box<dyn FnMut()>> = HashMap::new();
    let mut ui_map: HashMap<usize, UiEntry> = HashMap::new();

    let separator = PredefinedMenuItem::separator();
    {
        let mi = MenuItem::new(&cfg.title, true, None);
        menu.append(&mi).unwrap();
        menu.append(&separator).unwrap();
    }
    for (index, item) in cfg.groups.iter().enumerate() {
        // menu.append(&separator).unwrap();

        let sub = Submenu::new(&item.name, true);

        {
            let mi = MenuItem::new(&item.name, true, None);
            sub.append(&mi).unwrap();
            sub.append(&separator).unwrap();
        }

        let start_id = format!("{}_start", index);
        let stop_id = format!("{}_stop", index);
        let restart_id = format!("{}_restart", index);

        let start = MenuItem::with_id(&start_id, lang_text("启动", "Start"), true, None);
        let stop = MenuItem::with_id(&stop_id, lang_text("停止", "Stop"), false, None);
        let restart = MenuItem::with_id(&restart_id, lang_text("重启", "Restart"), false, None);

        sub.append(&start).unwrap();
        sub.append(&stop).unwrap();
        sub.append(&restart).unwrap();

        let mut open_browser: Option<IconMenuItem> = None;
        {
            let open_id = format!("{}_browser", index);
            if !item.address.is_empty() {
                sub.append(&separator).unwrap();
                {
                    let mi = IconMenuItem::with_id(&open_id, lang_text("打开", "Browser"), false, None, None);
                    sub.append(&mi).unwrap();
                    open_browser = Some(mi.clone());
                }

                {
                    let item_arc = Arc::new(item.clone());
                    let s = state.clone();
                    actions.insert(open_id, Box::new(move || item_arc.open_browser(&s)));
                }
            }
        }

        menu.append(&sub).unwrap();
        {
            let ui = UiEntry {
                start: start.clone(),
                stop: stop.clone(),
                restart: restart.clone(),
                title: TitleMenu::Submenu(sub),
                open: open_browser,
            };
            ui.set_running(false);
            ui_map.insert(item.uniq_id, ui);
        }

        let item_arc = Arc::new(item.clone());
        let s = state.clone();
        actions.insert(start_id, Box::new(move || item_arc.start(&s)));

        let item_arc = Arc::new(item.clone());
        let s = state.clone();
        actions.insert(stop_id, Box::new(move || item_arc.stop(&s)));

        let item_arc = Arc::new(item.clone());
        let s = state.clone();
        actions.insert(restart_id, Box::new(move || item_arc.restart(&s)));
    }

    // ===== Quit =====
    {
        let state_quit = state.clone();
        actions.insert(EVENT_QUIT.to_string(), Box::new(move || state_quit.stop_all()));

        menu.append(&separator).unwrap();
        let quit_item = MenuItem::with_id(EVENT_QUIT, lang_text("退出", "Quit"), true, None);
        menu.append(&quit_item).unwrap();
    }

    let _tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_icon(cfg.load_icon())
        .with_tooltip(cfg.title.clone())
        .build()
        .unwrap();

    // auto start
    for item in &cfg.groups {
        if item.auto_start {
            item.start(&state);

            if item.auto_open {
                item.open_browser(&state);
            }
        }
    }

    event_loop.run(move |_event, _, control_flow| {
        *control_flow = ControlFlow::Wait;

        while let Ok((id, running)) = rx.try_recv() {
            eprintln!("receive tx ({},{})", id, running);
            if let Some(ui) = ui_map.get_mut(&id) {
                ui.set_running(running);
            }
        }

        if let Ok(event) = MenuEvent::receiver().try_recv() {
            let eid = event.id.0.as_str();
            eprintln!("receive event {:?} , eid={})", event, eid);
            if let Some(action) = actions.get_mut(eid) {
                eprintln!("action executed");
                action();
            }

            if eid == EVENT_QUIT {
                *control_flow = ControlFlow::Exit;
                return;
            }
        }
    });
}

// ================= config =================

fn load_cfg() -> anyhow::Result<AppConfig> {
    let args = Args::parse();

    let cfg = config::Config::builder()
        .add_source(config::File::with_name(&args.config))
        .build()?;

    Ok(cfg.try_deserialize()?)
}
