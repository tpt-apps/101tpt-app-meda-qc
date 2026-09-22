//! Fuzz media-boundary handling ([spec § 24.4]; CLI `app`/`watch` crates).
//!
//! Arbitrary file names (including NUL bytes and separator characters) flow
//! through the media-extension filter and the watch event classifier. Both
//! must be total and panic-free on any path.

#![no_main]

use libfuzzer_sys::fuzz_target;
use notify::event::{CreateKind, ModifyKind, RenameMode, DataChange};
use notify::EventKind;

fuzz_target!(|data: &[u8]| {
    let name = String::from_utf8_lossy(data);
    let _ = tpt_app_media_qc_cli::app::is_media_file(std::path::Path::new(name.as_ref()));
    // Prefix bytes onto a known media extension to test case/separator edges.
    if name.contains('.') {
        let _ = tpt_app_media_qc_cli::app::is_media_file(
            std::path::Path::new("a/b")
                .join(name.as_ref())
                .join("shot.mp4")
                .as_path(),
        );
    }

    // Watch event classification: every input byte steers the kind.
    let kind = match data.first().copied().unwrap_or(0) % 6 {
        0 => EventKind::Create(CreateKind::File),
        1 => EventKind::Modify(ModifyKind::Name(RenameMode::To)),
        2 => EventKind::Modify(ModifyKind::Data(DataChange::Size)),
        3 => EventKind::Create(CreateKind::Folder),
        4 => EventKind::Remove(notify::event::RemoveKind::File),
        _ => EventKind::Access(notify::event::AccessKind::Read),
    };
    let _ = tpt_app_media_qc_cli::watch::is_new_file_event(&kind);
});