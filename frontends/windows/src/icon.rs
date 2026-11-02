use eframe::egui;

const TRAY_PNG: &[u8] = include_bytes!("../../../assets/icons/voime-32.png");
const WINDOW_PNG: &[u8] = include_bytes!("../../../assets/icons/voime-256.png");

pub fn tray_png() -> &'static [u8] {
    TRAY_PNG
}

pub fn window_icon() -> egui::IconData {
    let image = image::load_from_memory(WINDOW_PNG)
        .expect("embedded window icon must be a valid PNG")
        .into_rgba8();
    let (width, height) = image.dimensions();
    egui::IconData {
        rgba: image.into_raw(),
        width,
        height,
    }
}

#[cfg(test)]
mod tests {
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateIconFromResourceEx, DestroyIcon, LR_DEFAULTCOLOR,
    };

    #[test]
    fn embedded_tray_icon_loads() {
        let icon = unsafe {
            CreateIconFromResourceEx(
                super::tray_png(),
                true,
                0x0003_0000,
                32,
                32,
                LR_DEFAULTCOLOR,
            )
        }
        .expect("embedded tray icon must load");
        unsafe {
            let _ = DestroyIcon(icon);
        }
    }

    #[test]
    fn window_icon_decodes() {
        let icon = super::window_icon();
        assert_eq!((icon.width, icon.height), (256, 256));
        assert_eq!(icon.rgba.len(), 256 * 256 * 4);
    }
}
