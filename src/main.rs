#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use clap::Parser;
use command_group::{CommandGroup, GroupChild};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::process::Command;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tray_icon::{
    TrayIconBuilder,
    menu::{Icon, IconMenuItem, IsMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu},
};

#[derive(Parser, Debug)]
#[command(author, version, about)]
struct Args {
    #[arg(short, long, value_name = "c", default_value = "conf/tray.yaml")]
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
    address: String,

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

struct ProcessInfo {
    process: Option<GroupChild>,
}

enum TitleMenu {
    Submenu(Submenu),
    Icon(IconMenuItem),
}

struct AppMenu {
    title: Option<Arc<TitleMenu>>,
    start: Arc<MenuItem>,
    stop: Arc<MenuItem>,
    restart: Arc<MenuItem>,
    open: Option<Arc<MenuItem>>,
}

struct AppEntry {
    proc: Mutex<ProcessInfo>,
    menu: Mutex<AppMenu>,
}

struct AppState {
    config: AppConfig,
    apps: Mutex<HashMap<usize, Arc<AppEntry>>>,
}

impl AppState {
    fn with_process<F, R>(&self, id: usize, f: F) -> Option<R>
    where
        F: FnOnce(&mut GroupChild) -> R,
    {
        let apps = self.apps.lock().unwrap();

        let entry = apps.get(&id)?;
        let mut proc = entry.proc.lock().unwrap();

        let child = proc.process.as_mut()?;
        Some(f(child))
    }

    fn with_menu<F, R>(&self, id: usize, f: F) -> Option<R>
    where
        F: FnOnce(&mut AppMenu) -> R,
    {
        let mut apps = self.apps.lock().unwrap();

        let entry = apps.get_mut(&id)?;

        let mut menu = entry.menu.lock().unwrap();
        Some(f(&mut *menu))
    }

    fn update_ui(&self, id: usize, running: bool) {
        self.with_menu(id, |menu| {
            menu.start.set_enabled(!running);
            menu.stop.set_enabled(running);
            menu.restart.set_enabled(running);
            if let Some(item) = &menu.open {
                item.set_enabled(running);
            }

            if let Some(title) = &menu.title {
                match title.as_ref() {
                    TitleMenu::Submenu(title_menu) => {
                        // submenu: &Submenu
                        // title_menu.set_enabled(!running);
                        title_menu.set_icon(Some(get_icon(running)));
                    }
                    TitleMenu::Icon(title_menu) => {
                        // icon: &IconMenuItem
                        // title_menu.set_enabled(!running);
                        title_menu.set_icon(Some(get_icon(running)));
                    }
                }
            }
        });
    }

    fn set_title_menu(&self, id: usize, title: Arc<TitleMenu>) {
        self.with_menu(id, |menu| {
            menu.title = Some(title);
        });
    }

    fn is_running(&self, id: usize) -> bool {
        let apps = self.apps.lock().unwrap();

        let entry = match apps.get(&id) {
            Some(e) => e,
            None => return false,
        };

        let mut proc = entry.proc.lock().unwrap();

        if let Some(child) = proc.process.as_mut() {
            match child.try_wait() {
                Ok(Some(_status)) => {
                    // 进程已经退出，清空
                    proc.process = None;
                    false
                }
                Ok(None) => {
                    // 还在运行
                    true
                }
                Err(_) => {
                    // 出错，当作没运行
                    proc.process = None;
                    false
                }
            }
        } else {
            false
        }
    }

    fn update_proc(&self, id: usize, info: Option<GroupChild>) {
        let mut apps = self.apps.lock().unwrap();

        if let Some(entry) = apps.get_mut(&id) {
            let mut proc = entry.proc.lock().unwrap();
            proc.process = info;
        }
    }

    fn stop_all(self: &Arc<Self>) {
        eprintln!("stopping all apps ...");
        for item in self.config.groups.clone() {
            item.stop(&self.clone());
        }
    }

    fn auto_start(self: &Arc<Self>) {
        eprintln!("auto staring  apps ...");
        for item in self.config.groups.clone() {
            self.update_ui(item.uniq_id, false); // 初始化状态
            if item.auto_start {
                item.start(&self.clone());
            }
        }
    }
}

fn build_shell(cmd: &str) -> Command {
    if cfg!(target_os = "windows") {
        let mut c = Command::new("cmd");
        c.args(["/C", cmd]);
        c
    } else {
        let mut c = Command::new("sh");
        c.args(["-c", cmd]);
        c
    }
}

impl AppItem {
    fn start(&self, state: &Arc<AppState>) {
        eprintln!("process {} starting ...", self.name);

        if state.is_running(self.uniq_id) {
            eprintln!("process {} already is running, skipped", self.name);
            return;
        }

        let mut cmd = build_shell(&self.start);

        if !self.home.is_empty() {
            cmd.current_dir(self.home.as_str());
        }

        match cmd.group_spawn() {
            Ok(group) => {
                println!("start {}, pid={}", &self.name, group.id());
                state.update_proc(self.uniq_id, Some(group));
            }
            Err(e) => {
                eprintln!(
                    "Failed to start [{}]: {}, home={} cmd={:?}",
                    self.name,
                    e,
                    self.home.as_str(),
                    cmd
                );
                return;
            }
        }

        state.update_ui(self.uniq_id, true);
    }

    fn stop(&self, state: &Arc<AppState>) {
        eprintln!("process {} stopping ...", self.name);
        if !state.is_running(self.uniq_id) {
            eprintln!("process {} not running, skipped", self.name);
            return;
        }

        if !self.stop.is_empty() {
            let mut cmd = build_shell(self.stop.as_str());
            if !self.home.is_empty() {
                cmd.current_dir(self.home.as_str());
            }
            let ret = cmd.status();
            eprintln!("process {} stopped with status {:?}", self.name, ret);
        }
        state.with_process(self.uniq_id, |child| {
            let r1 = child.kill();
            let r2 = child.wait(); // 防 zombie
            eprintln!("process {} kill with status {:?} -> {:?}", self.name, r1, r2);
        });

        if state.is_running(self.uniq_id) {
            return;
        }

        state.update_ui(self.uniq_id, false);
    }

    fn restart(&self, state: &Arc<AppState>) {
        self.stop(state);
        self.start(state);
    }
}

fn load_cfg() -> anyhow::Result<AppConfig> {
    let args = Args::parse();
    let cfg_path = args.config.as_str();
    eprintln!("using config {}", cfg_path);

    let settings = config::Config::builder()
        .add_source(config::File::with_name(cfg_path))
        .build()?;

    let cfg: AppConfig = settings.try_deserialize()?;
    Ok(cfg)
}

fn get_icon(running: bool) -> Icon {
    if running {
        return Icon::from_rgba(circle_rgba(0, 200, 0), 32, 32).unwrap();
    }
    return Icon::from_rgba(circle_rgba(200, 0, 0), 32, 32).unwrap();
}

fn circle_rgba(r: u8, g: u8, b: u8) -> Vec<u8> {
    let mut pixels = vec![0u8; 32 * 32 * 4];

    for y in 0..32 {
        for x in 0..32 {
            let dist = ((x as f32 - 16.0).powi(2) + (y as f32 - 16.0).powi(2)).sqrt();
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

trait MenuLike {
    fn add_item(&mut self, item: &dyn IsMenuItem);
}

impl MenuLike for Menu {
    fn add_item(&mut self, item: &dyn IsMenuItem) {
        self.append(item).unwrap();
    }
}

impl MenuLike for Submenu {
    fn add_item(&mut self, item: &dyn IsMenuItem) {
        self.append(item).unwrap();
    }
}

fn main() {
    let cfg = load_cfg().unwrap();
    println!("cfg->  {:#?}", &cfg);

    let event_loop = EventLoopBuilder::new().build();

    let mut actions: HashMap<String, Box<dyn FnMut()>> = HashMap::new();

    let mut add_menu_item = |menu: &mut dyn MenuLike, label: &str, id: &str, f: Box<dyn FnMut()>| {
        let item = MenuItem::with_id(id, label, true, None);
        menu.add_item(&item);
        actions.insert(id.to_string(), f);
        return item;
    };

    let state = Arc::new(AppState {
        apps: Mutex::new(HashMap::new()),
        config: cfg.clone(),
    });

    let mut menu = Menu::new();
    let separator = PredefinedMenuItem::separator();
    {
        let mi = MenuItem::new(&cfg.title, true, None);
        menu.append(&mi).unwrap();
    }

    let mut add_app = |menu: &mut dyn MenuLike, index: usize, item: AppItem| {
        let item_arc = Arc::new(item.clone());

        let item_start = Arc::clone(&item_arc);
        let state_start = Arc::clone(&state);
        let start_item = add_menu_item(
            menu,
            "Start",
            &format!("{}_start", index),
            Box::new(move || {
                item_start.start(&state_start);
            }),
        );

        let item_stop = Arc::clone(&item_arc);
        let state_stop = Arc::clone(&state);
        let stop_item = add_menu_item(
            menu,
            "Stop",
            &format!("{}_stop", index),
            Box::new(move || {
                item_stop.stop(&state_stop);
            }),
        );
        stop_item.set_enabled(false);

        let item_restart = Arc::clone(&item_arc);
        let state_restart = Arc::clone(&state);
        let restart_item = add_menu_item(
            menu,
            "Restart",
            &format!("{}_restart", index),
            Box::new(move || {
                item_restart.restart(&state_restart);
            }),
        );
        restart_item.set_enabled(false);

        let mut open_item: Option<Arc<MenuItem>> = None;
        {
            let item_open = Arc::clone(&item_arc);
            if !item_open.address.is_empty() {
                let item = add_menu_item(
                    menu,
                    "Browser",
                    &format!("{}_open_browser", index),
                    Box::new(move || {
                        eprintln!("{} open browser ...", &item_open.name);
                        webbrowser::open(&item_open.address).unwrap();
                    }),
                );
                item.set_enabled(false);
                open_item = Some(Arc::new(item));
            }
        }

        let app_menu = AppMenu {
            title: None,
            start: Arc::new(start_item),
            stop: Arc::new(stop_item),
            restart: Arc::new(restart_item),
            open: open_item,
        };
        let app_info = AppEntry {
            menu: Mutex::new(app_menu),
            proc: Mutex::new(ProcessInfo { process: None }),
        };
        state.apps.lock().unwrap().insert(item_arc.uniq_id, Arc::new(app_info));
    };

    for (index, item) in cfg.groups.iter().enumerate() {
        if cfg.groups.len() > 1 {
            if index == 0 {
                menu.append(&separator).unwrap();
            }
            let mut sub_menu = tray_icon::menu::Submenu::new(&item.name, true);
            {
                let mi = IconMenuItem::new(&item.name, true, None, None);
                sub_menu.append(&mi).unwrap();
                sub_menu.append(&separator).unwrap();
            }

            add_app(&mut sub_menu, index, item.clone());
            menu.append(&sub_menu).unwrap();

            state
                .clone()
                .set_title_menu(item.uniq_id, Arc::new(TitleMenu::Submenu(sub_menu)));
        } else {
            menu.append(&separator).unwrap();
            let mi = IconMenuItem::new(&item.name, true, None, None);
            menu.append(&mi).unwrap();
            menu.append(&separator).unwrap();

            add_app(&mut menu, index, item.clone());
            state
                .clone()
                .set_title_menu(item.uniq_id, Arc::new(TitleMenu::Icon(mi)));
        }
    }

    menu.append(&separator).unwrap();
    let state_quit = Arc::clone(&state);
    add_menu_item(
        &mut menu,
        "Quit",
        "quit",
        Box::new(move || {
            state_quit.stop_all();
            std::process::exit(0)
        }),
    );

    let _tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_icon(cfg.load_icon())
        .with_tooltip(cfg.title)
        .build()
        .unwrap();

    state.auto_start();

    event_loop.run(move |_event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        if let Ok(event) = MenuEvent::receiver().try_recv() {
            if let Some(action) = actions.get_mut(event.id.0.as_str()) {
                action();
            }
        }
    });
}

fn load_icon() -> tray_icon::Icon {
    let (icon_rgba, icon_width, icon_height) = {
        let image = image::load_from_memory(include_bytes!("../icons/tray.png"))
            .expect("Failed to load icon")
            .into_rgba8();
        let (width, height) = image.dimensions();
        (image.into_raw(), width, height)
    };
    tray_icon::Icon::from_rgba(icon_rgba, icon_width, icon_height).expect("Failed to create icon")
}
