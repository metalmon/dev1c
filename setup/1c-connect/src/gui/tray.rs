//! Значок в трее и его меню. Заглушка задачи 3: ничего не рисует и не
//! подписывается на события ОС — настоящий значок, меню и обработчики
//! добавит задача 5.

use super::Ev;
use futures::channel::mpsc::UnboundedSender;
use gpui_kit::App;

pub struct Tray;

impl Tray {
    pub fn set_state(&self, _up: bool, _on: bool) {}
}

pub fn build(_tx: UnboundedSender<Ev>, _choice: super::theme::Choice) -> anyhow::Result<Tray> {
    Ok(Tray)
}

pub fn on_tray(_ev: tray_icon::TrayIconEvent, _cx: &mut App) {}

pub fn on_menu(_ev: tray_icon::menu::MenuEvent, _cx: &mut App) {}
