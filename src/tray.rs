//! 托盘图标与右键菜单的构建，以及菜单项与运行状态的联动。

use std::collections::HashMap;
use std::sync::Arc;

use tray_icon::{
    TrayIcon, TrayIconBuilder,
    menu::{IconMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu},
};

use crate::config::AppConfig;
use crate::i18n::lang_text;
use crate::state::AppState;

/// 退出菜单项的固定 id。
pub const EVENT_QUIT: &str = "quit";

/// 与单个应用条目对应的菜单项集合，用于在运行时切换可用状态 / 状态图标。
pub struct UiEntry {
    pub start: MenuItem,
    pub stop: MenuItem,
    pub restart: MenuItem,
    pub title: TitleMenu,
}

pub enum TitleMenu {
    Submenu(Submenu),
}

impl UiEntry {
    pub fn set_running(&self, running: bool) {
        self.start.set_enabled(!running);
        self.stop.set_enabled(running);
        self.restart.set_enabled(running);

        match &self.title {
            TitleMenu::Submenu(title_menu) => {
                title_menu.set_icon(Some(crate::icon::get_menu_icon(running)));
            }
        }
    }
}

/// `build_tray` 的返回值。
///
/// `tray` 必须保持存活（否则托盘图标会被移除），`actions` 与 `ui_map` 供事件循环使用。
pub struct TrayUi {
    /// 必须保持存活，否则托盘图标会被移除（仅持有、不读取，故允许 dead_code）。
    #[allow(dead_code)]
    pub tray: TrayIcon,
    pub actions: HashMap<String, Box<dyn FnMut()>>,
    pub ui_map: HashMap<usize, UiEntry>,
}

/// 根据配置与共享状态构建托盘图标、菜单，并返回菜单交互所需的映射。
pub fn build_tray(cfg: &AppConfig, state: &Arc<AppState>) -> TrayUi {
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
        if item.hidden {
            continue;
        }
        // menu.append(&separator).unwrap();

        let sub = Submenu::new(&item.name, true);

        {
            let mi = IconMenuItem::new(format!("  {}", &item.name), true, None, None);
            sub.append(&mi).unwrap();
            sub.append(&separator).unwrap();
        }

        // let mut open_browser: Option<IconMenuItem> = None;
        {
            let open_id = format!("{}_browser", index);
            if !item.address.is_empty() {
                {
                    let mi = MenuItem::with_id(&open_id, lang_text("🌐  打  开", "🌐  Open"), true, None);
                    sub.append(&mi).unwrap();
                    // open_browser = Some(mi.clone());
                }
                sub.append(&separator).unwrap();

                {
                    let item_arc = Arc::new(item.clone());
                    let s = state.clone();
                    actions.insert(open_id, Box::new(move || item_arc.open_browser(&s)));
                }
            }
        }

        {
            let start_id = format!("{}_start", index);
            let stop_id = format!("{}_stop", index);
            let restart_id = format!("{}_restart", index);

            let start = MenuItem::with_id(&start_id, lang_text(" ▶️   启  动", " ▶️  Start"), true, None);
            let stop = MenuItem::with_id(&stop_id, lang_text("⏹️  停  止", "⏹️  Stop"), false, None);
            let restart = MenuItem::with_id(&restart_id, lang_text("🔁  重  启", "🔁  Restart"), false, None);

            sub.append(&start).unwrap();
            sub.append(&stop).unwrap();
            sub.append(&restart).unwrap();

            let item_arc = Arc::new(item.clone());
            let s = state.clone();
            actions.insert(start_id, Box::new(move || item_arc.menu_start(&s)));

            let item_arc = Arc::new(item.clone());
            let s = state.clone();
            actions.insert(stop_id, Box::new(move || item_arc.menu_stop(&s)));

            let item_arc = Arc::new(item.clone());
            let s = state.clone();
            actions.insert(restart_id, Box::new(move || item_arc.menu_restart(&s)));

            {
                let ui = UiEntry {
                    start: start.clone(),
                    stop: stop.clone(),
                    restart: restart.clone(),
                    title: TitleMenu::Submenu(sub.clone()),
                    // open: open_browser,
                };
                ui.set_running(item.auto_start);
                ui_map.insert(item.uniq_id, ui);
            }
        }

        {
            for (si, ss) in item.opens.iter().enumerate() {
                if si == 0 {
                    sub.append(&separator).unwrap();
                }
                let bb_id = format!("{}_open_uri_{}", index, si);
                let mi = IconMenuItem::with_id(&bb_id, format!("↗️ {}", &ss.title), true, None, None);
                sub.append(&mi).unwrap();

                let ss_arc = Arc::new(ss.clone());
                actions.insert(bb_id, Box::new(move || ss_arc.open()));
            }
        }

        menu.append(&sub).unwrap();
    } // end for groups

    // ===== Quit =====
    {
        let state_quit = state.clone();
        let cv = cfg.clone();
        actions.insert(
            EVENT_QUIT.to_string(),
            Box::new(move || {
                cv.stop_all(&state_quit);
            }),
        );

        menu.append(&separator).unwrap();
        let quit_item = MenuItem::with_id(EVENT_QUIT, lang_text("❌  退   出", "❌ Quit"), true, None);
        menu.append(&quit_item).unwrap();
    }

    let tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_icon(cfg.load_icon())
        .with_tooltip(cfg.title.clone())
        .build()
        .unwrap();

    TrayUi { tray, actions, ui_map }
}
