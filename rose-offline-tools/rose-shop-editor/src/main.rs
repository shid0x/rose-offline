mod app;

use std::path::PathBuf;

use clap::{Arg, Command};
use rfd::{FileDialog, MessageButtons, MessageDialog, MessageDialogResult, MessageLevel};

enum LaunchMode {
    PackedDataIdx,
    ExtractedData,
}

fn main() -> eframe::Result<()> {
    let command = Command::new("rose-shop-editor")
        .about("ROSE NPC shop editor")
        .arg(
            Arg::new("input-path")
                .long("input-path")
                .help("Path to the ROSE installation root, data.idx, extracted data root, or extracted 3DDATA folder")
                .takes_value(true),
        );
    let matches = command.get_matches();

    let input_path = match matches.value_of("input-path").map(PathBuf::from) {
        Some(input_path) => input_path,
        None => match prompt_for_input_path() {
            Some(path) => path,
            None => return Ok(()),
        },
    };

    let native_options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1680.0, 960.0])
            .with_min_inner_size([1400.0, 820.0]),
        ..Default::default()
    };
    eframe::run_native(
        "ROSE NPC Shop Editor",
        native_options,
        Box::new(move |cc| Box::new(app::ShopEditorApp::load(input_path.clone(), &cc.egui_ctx))),
    )
}

fn prompt_for_input_path() -> Option<PathBuf> {
    let launch_mode = prompt_for_launch_mode()?;
    let current_dir = std::env::current_dir().ok();

    match launch_mode {
        LaunchMode::PackedDataIdx => {
            let mut dialog = FileDialog::new()
                .set_title("Select ROSE data.idx")
                .add_filter("ROSE data index", &["idx"])
                .set_file_name("data.idx");

            if let Some(current_dir) = current_dir {
                dialog = dialog.set_directory(current_dir);
            }

            dialog.pick_file()
        }
        LaunchMode::ExtractedData => {
            let mut dialog =
                FileDialog::new().set_title("Select extracted ROSE data root or 3DDATA folder");
            if let Some(current_dir) = current_dir {
                dialog = dialog.set_directory(current_dir);
            }

            dialog.pick_folder()
        }
    }
}

fn prompt_for_launch_mode() -> Option<LaunchMode> {
    let result = MessageDialog::new()
        .set_level(MessageLevel::Info)
        .set_title("Choose Shop Editor Source")
        .set_description(
            "Yes = open packed data.idx\nNo = open a pre-extracted data folder\nCancel = quit",
        )
        .set_buttons(MessageButtons::YesNoCancel)
        .show();

    match result {
        MessageDialogResult::Yes => Some(LaunchMode::PackedDataIdx),
        MessageDialogResult::No => Some(LaunchMode::ExtractedData),
        _ => None,
    }
}
