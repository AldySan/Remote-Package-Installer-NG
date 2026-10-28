//! Native file/folder dialogs on a helper thread.
//!
//! `rfd`'s async dialog needs a running async runtime; the UI thread has none,
//! so each dialog runs on its own single-threaded runtime and reports back
//! through a channel that the UI polls per frame (non-blocking).

use std::path::PathBuf;
use std::sync::mpsc;

pub enum DialogOutcome {
    Files(Vec<PathBuf>),
    Folder(PathBuf),
    Cancelled,
}

pub struct DialogPick {
    rx: mpsc::Receiver<DialogOutcome>,
}

impl DialogPick {
    /// Non-blocking check; `true` when the dialog finished.
    pub fn try_result(&self) -> Option<DialogOutcome> {
        self.rx.try_recv().ok()
    }
}

/// Show a multi-select PKG file dialog.
pub fn pick_pkg_files() -> DialogPick {
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("rpi-ng-dialog".into())
        .spawn(move || {
            let Ok(rt) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            else {
                let _ = tx.send(DialogOutcome::Cancelled);
                return;
            };
            rt.block_on(async move {
                let handle = rfd::AsyncFileDialog::new()
                    .set_title("Select PKG files")
                    .add_filter("PlayStation packages", &["pkg"])
                    .pick_files()
                    .await;
                let outcome = match handle {
                    Some(files) if !files.is_empty() => DialogOutcome::Files(
                        files.into_iter().map(|f| f.path().to_path_buf()).collect(),
                    ),
                    _ => DialogOutcome::Cancelled,
                };
                let _ = tx.send(outcome);
            });
        })
        .expect("dialog thread");
    DialogPick { rx }
}

/// Show a folder-pick dialog.
pub fn pick_folder() -> DialogPick {
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("rpi-ng-dialog".into())
        .spawn(move || {
            let Ok(rt) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            else {
                let _ = tx.send(DialogOutcome::Cancelled);
                return;
            };
            rt.block_on(async move {
                let handle = rfd::AsyncFileDialog::new()
                    .set_title("Select a folder with packages")
                    .pick_folder()
                    .await;
                let outcome = match handle {
                    Some(folder) => DialogOutcome::Folder(folder.path().to_path_buf()),
                    _ => DialogOutcome::Cancelled,
                };
                let _ = tx.send(outcome);
            });
        })
        .expect("dialog thread");
    DialogPick { rx }
}
