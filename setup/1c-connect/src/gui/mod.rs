//! Окно и трей на GPUI (gpui-kit) + tray-icon. Каркас: пустое окно
//! 380×560, без функций старой Tauri-версии — их возвращают следующие
//! задачи плана.

use crate::cfg::Cfg;
use common::kit::Kit;
use gpui_kit::prelude::*;
use gpui_kit::*;

pub const TRAY_PNG: &[u8] = include_bytes!("../../icons/tray.png");

/// Значок трея со статусом: кружок в правом нижнем углу, зелёный —
/// подключён; без связи значок обесцвечен и кружок серый.
pub fn badge(base: &image::RgbaImage, up: bool) -> image::RgbaImage {
    let (w, h) = base.dimensions();
    let mut out = base.clone();
    if !up {
        for p in out.pixels_mut() {
            let [r, g, b, _] = p.0;
            let gr = ((r as u32 * 30 + g as u32 * 59 + b as u32 * 11) / 100) as u8;
            p.0[0] = gr;
            p.0[1] = gr;
            p.0[2] = gr;
        }
    }
    let r = (w as f32 * 0.22).max(3.0);
    let (cx, cy) = (w as f32 - r - 1.0, h as f32 - r - 1.0);
    let colour: [u8; 4] = if up { [0x2f, 0x8f, 0x4e, 255] } else { [0x8d, 0x86, 0x7e, 255] };
    for y in 0..h {
        for x in 0..w {
            let d = ((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2)).sqrt();
            if d <= r {
                let rim = d > r - 1.5;
                out.put_pixel(x, y, image::Rgba(if rim { [255, 255, 255, 255] } else { colour }));
            }
        }
    }
    out
}

pub fn web_url(cfg: &Cfg) -> String {
    let port = cfg.web_port();
    let host = if port == 80 { "localhost".to_string() } else { format!("localhost:{port}") };
    format!("http://{host}{}", cfg.publication())
}

struct Panel;

impl Render for Panel {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full()
    }
}

/// Временный запуск: пустое окно 380×560. Трей, события и содержимое
/// приходят следующими задачами плана.
pub fn launch(_kit: Kit) -> anyhow::Result<()> {
    gpui_kit::application().with_assets(gpui_kit::assets::Assets).run(|cx| {
        gpui_kit::init(cx);
        // Окно закрыто — процесс живёт, пока «Выход» в трее (следующая задача).
        cx.set_quit_mode(QuitMode::Explicit);
        let opts = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, size(px(380.0), px(560.0)), cx))),
            is_resizable: false,
            ..Default::default()
        };
        let _ = gpui_kit::open_window(opts, cx, |_window, cx| cx.new(|_cx| Panel));
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{badge, web_url, TRAY_PNG};
    use crate::cfg::Cfg;

    #[test]
    fn the_badge_sits_in_the_corner_and_says_up_or_down() {
        let base = image::load_from_memory(TRAY_PNG).unwrap().to_rgba8();
        let up = badge(&base, true);
        assert_eq!(up.dimensions(), base.dimensions());
        let w = up.width() as usize;
        let at = |img: &image::RgbaImage, x: usize, y: usize| img.get_pixel(x as u32, y as u32).0[..3].to_vec();
        let c = w - 1 - (w as f32 * 0.22) as usize;
        assert_eq!(at(&up, c, c), vec![0x2f, 0x8f, 0x4e]);
        let down = badge(&base, false);
        assert_eq!(at(&down, c, c), vec![0x8d, 0x86, 0x7e]);
        let p = at(&down, 2, 2);
        assert!(p[0] == p[1] && p[1] == p[2], "{p:?}");
    }

    #[test]
    fn web_client_url_follows_the_publication() {
        let mut c = Cfg::default();
        assert_eq!(web_url(&c), "http://localhost/Buh/");
        c.host.base_url = "http://localhost:8081/InfoBase/hs/mcp-1c".into();
        assert_eq!(web_url(&c), "http://localhost:8081/InfoBase/");
    }
}
