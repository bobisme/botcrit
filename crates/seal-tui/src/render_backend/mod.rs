//! Rendering backend facade.
//!
//! The hard-switch to ftui is complete; this module exposes a stable app-facing
//! API backed by the ftui runtime and rendering stack.

pub use ftui_render::cell::PackedRgba;
mod ftui_compat;
pub use ftui_compat::*;

#[cfg(test)]
mod tests {
    use super::{
        color_blend_over, color_lerp, color_luminance, color_with_alpha, event_from_ftui,
        packed_to_rgba, rgba_to_packed,
    };
    use crate::render_backend::{Event, KeyCode, KeyModifiers, MouseEventKind, Rgba};

    #[test]
    fn packed_roundtrip_preserves_rgba_u8_components() {
        let source = Rgba::from_rgba_u8(12, 34, 56, 78);
        let packed = rgba_to_packed(source);
        let roundtrip = packed_to_rgba(packed);
        assert_eq!(source.to_rgba_u8(), roundtrip.to_rgba_u8());
    }

    #[test]
    fn packed_roundtrip_preserves_theme_seed_like_values() {
        let samples = [
            Rgba::from_rgba_u8(26, 26, 46, 255),
            Rgba::from_rgba_u8(122, 162, 247, 255),
            Rgba::from_rgba_u8(158, 206, 106, 255),
            Rgba::from_rgba_u8(247, 118, 142, 200),
        ];

        for color in samples {
            let roundtrip = packed_to_rgba(rgba_to_packed(color));
            assert_eq!(color.to_rgba_u8(), roundtrip.to_rgba_u8());
        }
    }

    #[test]
    fn color_helpers_keep_expected_semantics() {
        let base = Rgba::from_rgba_u8(30, 60, 90, 255);
        let tinted = color_with_alpha(base, 0.5);
        assert_eq!(tinted.to_rgba_u8().3, 128);

        let mixed = color_lerp(Rgba::BLACK, Rgba::WHITE, 0.5);
        let (r, g, b, a) = mixed.to_rgba_u8();
        assert_eq!((r, g, b, a), (128, 128, 128, 255));

        let over = color_blend_over(
            Rgba::from_rgba_u8(255, 0, 0, 128),
            Rgba::from_rgba_u8(0, 0, 255, 255),
        );
        let (_, _, _, alpha) = over.to_rgba_u8();
        assert_eq!(alpha, 255);

        assert!((color_luminance(Rgba::WHITE) - 1.0).abs() < 1e-6);
        assert!((color_luminance(Rgba::BLACK) - 0.0).abs() < 1e-6);
    }

    #[test]
    fn event_mapping_from_ftui_handles_keyboard_and_focus() {
        use ftui_core::event::{
            Event as FtEvent, KeyCode as FtKeyCode, KeyEvent as FtKeyEvent, Modifiers,
        };

        let mapped = event_from_ftui(FtEvent::Key(
            FtKeyEvent::new(FtKeyCode::Char('c')).with_modifiers(Modifiers::CTRL),
        ));
        match mapped {
            Some(Event::Key(key)) => {
                assert_eq!(key.code, KeyCode::Char('c'));
                assert!(key.modifiers.contains(KeyModifiers::CTRL));
            }
            _ => panic!("expected key event"),
        }

        assert!(matches!(
            event_from_ftui(FtEvent::Focus(true)),
            Some(Event::FocusGained)
        ));
        assert!(matches!(
            event_from_ftui(FtEvent::Focus(false)),
            Some(Event::FocusLost)
        ));
    }

    #[test]
    fn event_mapping_from_ftui_handles_mouse_and_resize() {
        use ftui_core::event::{
            Event as FtEvent, MouseButton, MouseEvent, MouseEventKind as FtMouseEventKind,
        };

        let mouse = event_from_ftui(FtEvent::Mouse(MouseEvent::new(
            FtMouseEventKind::Down(MouseButton::Left),
            10,
            4,
        )));
        match mouse {
            Some(Event::Mouse(mouse)) => {
                assert_eq!(mouse.kind, MouseEventKind::Press);
                assert_eq!(mouse.x, 10);
                assert_eq!(mouse.y, 4);
            }
            _ => panic!("expected mouse event"),
        }

        let resize = event_from_ftui(FtEvent::Resize {
            width: 120,
            height: 40,
        });
        match resize {
            Some(Event::Resize(event)) => {
                assert_eq!(event.width, 120);
                assert_eq!(event.height, 40);
            }
            _ => panic!("expected resize event"),
        }
    }
}
