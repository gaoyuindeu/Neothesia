use std::{
    cell::{OnceCell, RefCell},
    rc::Rc,
    sync::Arc,
};

pub use glyphon::FontSystem;

thread_local! {
     static FONT_SYSTEM: OnceCell<Rc<RefCell<FontSystem>>> = const { OnceCell::new() };
}

/// Returns the global [`FontSystem`].
pub fn font_system() -> Rc<RefCell<FontSystem>> {
    FONT_SYSTEM.with(|system| {
        system
            .get_or_init(|| {
                let mut system = FontSystem::new_with_fonts([
                    glyphon::fontdb::Source::Binary(Arc::new(include_bytes!(
                        "../../assets/fonts/Iced-Icons.ttf"
                    ))),
                    glyphon::fontdb::Source::Binary(Arc::new(include_bytes!(
                        "../../assets/fonts/Roboto-Regular.ttf"
                    ))),
                    glyphon::fontdb::Source::Binary(Arc::new(include_bytes!(
                        "../../assets/fonts/bootstrap-icons.ttf"
                    ))),
                    glyphon::fontdb::Source::Binary(Arc::new(include_bytes!(
                        "../../assets/fonts/Leland.otf"
                    ))),
                ]);
                load_cjk_font(&mut system);
                Rc::new(RefCell::new(system))
            })
            .clone()
    })
}

/// File names and titles are often Chinese or Japanese: load one system CJK font so text
/// shaping can fall back to it (the bundled fonts only cover Latin)
fn load_cjk_font(system: &mut FontSystem) {
    const CANDIDATES: &[&str] = &[
        r"C:\Windows\Fonts\msyh.ttc",
        r"C:\Windows\Fonts\msyh.ttf",
        r"C:\Windows\Fonts\simsun.ttc",
        r"C:\Windows\Fonts\YuGothM.ttc",
        "/System/Library/Fonts/PingFang.ttc",
        "/System/Library/Fonts/Hiragino Sans GB.ttc",
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/google-noto-cjk/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/truetype/wqy/wqy-microhei.ttc",
    ];
    if let Some(path) = CANDIDATES
        .iter()
        .map(std::path::Path::new)
        .find(|p| p.exists())
        && let Err(err) = system.db_mut().load_font_file(path)
    {
        log::warn!("Could not load {}: {err}", path.display());
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn chinese_text_has_glyphs() {
        use glyphon::{Attrs, Buffer, Family, Metrics, Shaping};
        let system = super::font_system();
        let mut system = system.borrow_mut();
        let mut buffer = Buffer::new(&mut system, Metrics::new(14.0, 18.0));
        buffer.set_text(
            "月光奏鸣曲 Moonlight",
            &Attrs::new().family(Family::Name("Roboto")),
            Shaping::Advanced,
            None,
        );
        buffer.shape_until_scroll(&mut system, false);
        let glyphs: Vec<_> = buffer
            .layout_runs()
            .flat_map(|run| run.glyphs.iter().map(|g| g.glyph_id))
            .collect();
        assert!(!glyphs.is_empty());
        if std::path::Path::new(r"C:\Windows\Fonts\msyh.ttc").exists() {
            assert!(glyphs.iter().all(|&g| g != 0), "{glyphs:?}");
        }
    }
}
