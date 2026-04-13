use std::{
    collections::HashMap,
    io::Cursor,
    path::{Path, PathBuf},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{anyhow, Context};
use ddsfile::{D3DFormat, Dds, DxgiFormat};
use eframe::egui::{
    self, Button, ColorImage, ComboBox, RichText, ScrollArea, SidePanel, TextureHandle,
    TopBottomPanel,
};
use image::ImageFormat;
use rose_data::{
    ItemClass, ItemDatabase, ItemReference, ItemType, NpcDatabase, StringDatabase, ZoneDatabase,
    ZoneId, ZoneList,
};
use rose_data_irose::{
    decode_item_base1000, encode_item_type, get_item_database, get_npc_database,
    get_string_database, get_zone_database, get_zone_list,
};
use rose_file_readers::{
    RoseFile, RoseFileWriter, StbFile, TsiFile, TsiSprite, VfsIndex, VfsPathBuf, VirtualFilesystem,
};
use texpresso::Format as BcFormat;

const LIST_NPC_PATH: &str = "3DDATA/STB/LIST_NPC.STB";
const LIST_SELL_PATH: &str = "3DDATA/STB/LIST_SELL.STB";
const NPC_STORE_TAB_COLUMN_START: usize = 21;
const NPC_STORE_TAB_COUNT: usize = 4;
const SHOP_ITEM_COLUMN_START: usize = 2;
const SHOP_ITEM_SLOT_COUNT: usize = 48;

const ITEM_TYPES: [ItemType; 14] = [
    ItemType::Face,
    ItemType::Head,
    ItemType::Body,
    ItemType::Hands,
    ItemType::Feet,
    ItemType::Back,
    ItemType::Jewellery,
    ItemType::Weapon,
    ItemType::SubWeapon,
    ItemType::Consumable,
    ItemType::Gem,
    ItemType::Material,
    ItemType::Quest,
    ItemType::Vehicle,
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum SelectionMode {
    NpcList,
    Zone,
}

#[derive(Clone, Copy)]
struct ZoneNpcSummary {
    npc_id: u16,
    spawn_count: usize,
}

#[derive(Clone, Copy)]
struct ItemCatalogEntry {
    reference: ItemReference,
    class: ItemClass,
}

struct ItemIconTexture {
    handle: TextureHandle,
    size: egui::Vec2,
}

struct ItemIconAtlas {
    sprites: Vec<TsiSprite>,
    textures: Vec<ItemIconTexture>,
}

struct ItemIcon {
    texture_id: egui::TextureId,
    uv: egui::Rect,
}

#[derive(Clone, Copy, Debug)]
enum DdsTextureFormat {
    Compressed(BcFormat),
    A8R8G8B8,
    X8R8G8B8,
    R8G8B8,
    A4R4G4B4,
    R5G6B5,
    A1R5G5B5,
}

enum AppState {
    Ready(EditorData),
    Error(String),
}

#[derive(Clone, Copy)]
enum SourceKind {
    PackedDataIdx,
    ExtractedData,
}

impl SourceKind {
    fn label(self) -> &'static str {
        match self {
            SourceKind::PackedDataIdx => "Packed data.idx",
            SourceKind::ExtractedData => "Extracted 3DDATA",
        }
    }
}

enum SaveTarget {
    Packed {
        data_idx_path: PathBuf,
        writer_index: VfsIndex,
    },
    Extracted {
        root_path: PathBuf,
    },
}

struct ResolvedInputSource {
    source_kind: SourceKind,
    display_path: PathBuf,
    save_target: SaveTarget,
}

pub struct ShopEditorApp {
    state: AppState,
}

struct EditorData {
    input_path: PathBuf,
    source_kind: SourceKind,
    source_display_path: PathBuf,
    save_target: SaveTarget,
    string_database: Arc<StringDatabase>,
    item_database: Arc<ItemDatabase>,
    npc_database: Arc<NpcDatabase>,
    zone_list: Arc<ZoneList>,
    list_npc: StbFile,
    list_sell: StbFile,
    tab_usage: HashMap<u16, Vec<u16>>,
    shopkeeper_npc_ids: Vec<u16>,
    zone_ids: Vec<ZoneId>,
    zone_shopkeepers: HashMap<u16, Vec<ZoneNpcSummary>>,
    item_catalog: Vec<ItemCatalogEntry>,
    item_icons: Option<ItemIconAtlas>,
    selection_mode: SelectionMode,
    selected_npc_id: Option<u16>,
    selected_zone_id: Option<ZoneId>,
    active_tab_index: usize,
    npc_filter: String,
    zone_filter: String,
    item_filter: String,
    item_type_filter: Option<ItemType>,
    item_class_filter: Option<ItemClass>,
    dirty: bool,
    status_message: String,
}

impl ShopEditorApp {
    pub fn load(input_path: PathBuf, egui_ctx: &egui::Context) -> Self {
        let state = match EditorData::load(input_path, egui_ctx) {
            Ok(editor) => AppState::Ready(editor),
            Err(error) => AppState::Error(format!("{error:#}")),
        };
        Self { state }
    }
}

impl eframe::App for ShopEditorApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        match &mut self.state {
            AppState::Ready(editor) => editor.ui(ctx),
            AppState::Error(error) => {
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.heading("Failed to open shop editor");
                    ui.separator();
                    ui.label(error.as_str());
                });
            }
        }
    }
}

impl EditorData {
    fn load(input_path: PathBuf, egui_ctx: &egui::Context) -> Result<Self, anyhow::Error> {
        let resolved_input = resolve_input_source(&input_path)?;
        let vfs = create_virtual_filesystem(&resolved_input)?;

        let string_database = get_string_database(&vfs, 1)?;
        let item_database = Arc::new(get_item_database(&vfs, string_database.clone())?);
        let npc_database = Arc::new(get_npc_database(
            &vfs,
            string_database.clone(),
            &rose_data::NpcDatabaseOptions {
                load_frame_data: false,
            },
        )?);
        let zone_database = Arc::new(get_zone_database(&vfs, string_database.clone())?);
        let zone_list = Arc::new(get_zone_list(&vfs, string_database.clone())?);

        let list_npc = vfs.read_file::<StbFile, _>(LIST_NPC_PATH)?;
        let list_sell = vfs.read_file::<StbFile, _>(LIST_SELL_PATH)?;

        let shopkeeper_npc_ids = npc_database
            .iter()
            .filter(|npc| npc_sells_items(&list_npc, &list_sell, npc.id.get()))
            .map(|npc| npc.id.get())
            .collect::<Vec<_>>();

        let zone_shopkeepers = build_zone_shopkeepers(&zone_database, &list_npc, &list_sell);
        let mut zone_ids = zone_shopkeepers
            .keys()
            .filter_map(|zone_id| ZoneId::new(*zone_id))
            .collect::<Vec<_>>();
        zone_ids.sort_by_key(|zone_id| zone_id.get());

        let selected_zone_id = zone_ids.first().copied();
        let selected_npc_id = shopkeeper_npc_ids.first().copied();

        let item_catalog = build_item_catalog(&item_database);
        let (item_icons, status_message) = match load_item_icons(&vfs, egui_ctx) {
            Ok(item_icons) => (
                Some(item_icons),
                format!(
                    "Opened {} from {}",
                    resolved_input.source_kind.label(),
                    resolved_input.display_path.display()
                ),
            ),
            Err(error) => (
                None,
                format!(
                    "Opened {} from {}. Item icons unavailable: {}",
                    resolved_input.source_kind.label(),
                    resolved_input.display_path.display(),
                    error
                ),
            ),
        };

        Ok(Self {
            input_path,
            source_kind: resolved_input.source_kind,
            source_display_path: resolved_input.display_path,
            save_target: resolved_input.save_target,
            string_database,
            item_database,
            npc_database,
            zone_list,
            tab_usage: build_tab_usage(&list_npc),
            list_npc,
            list_sell,
            shopkeeper_npc_ids,
            zone_ids,
            zone_shopkeepers,
            item_catalog,
            item_icons,
            selection_mode: SelectionMode::NpcList,
            selected_npc_id,
            selected_zone_id,
            active_tab_index: 0,
            npc_filter: String::new(),
            zone_filter: String::new(),
            item_filter: String::new(),
            item_type_filter: None,
            item_class_filter: None,
            dirty: false,
            status_message,
        })
    }

    fn ui(&mut self, ctx: &egui::Context) {
        TopBottomPanel::top("shop_editor_top").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("ROSE NPC Shop Editor");
                ui.label(format!(
                    "{}: {}",
                    self.source_kind.label(),
                    self.source_display_path.display()
                ));
                if ui
                    .add_enabled(self.dirty, Button::new("Save Changes"))
                    .clicked()
                {
                    if let Err(error) = self.save() {
                        self.status_message = format!("Save failed: {error:#}");
                    }
                }
                if ui.button("Reload").clicked() {
                    match EditorData::load(self.input_path.clone(), ctx) {
                        Ok(reloaded) => *self = reloaded,
                        Err(error) => {
                            self.status_message = format!("Reload failed: {error:#}");
                        }
                    }
                }
            });

            ui.label(self.status_message.as_str());
        });

        SidePanel::left("shop_editor_left")
            .default_width(280.0)
            .show(ctx, |ui| self.draw_selector_panel(ui));

        SidePanel::right("shop_editor_right")
            .default_width(420.0)
            .show(ctx, |ui| self.draw_item_browser(ui));

        egui::CentralPanel::default().show(ctx, |ui| self.draw_shop_editor(ui));
    }

    fn draw_selector_panel(&mut self, ui: &mut egui::Ui) {
        ui.heading("Shopkeepers");
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.selection_mode, SelectionMode::NpcList, "NPC List");
            ui.selectable_value(&mut self.selection_mode, SelectionMode::Zone, "Zone");
        });
        ui.separator();

        match self.selection_mode {
            SelectionMode::NpcList => {
                ui.label("Filter NPC");
                ui.text_edit_singleline(&mut self.npc_filter);
                let filter = self.npc_filter.to_lowercase();
                let npc_ids = self
                    .shopkeeper_npc_ids
                    .iter()
                    .copied()
                    .filter(|npc_id| npc_matches_filter(*npc_id, &filter, &self.npc_database))
                    .collect::<Vec<_>>();

                let list_height = ui.available_height().max(200.0);
                ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), list_height),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ScrollArea::vertical()
                            .id_source("npc_list_scroll")
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                for npc_id in npc_ids {
                                    let label = self.npc_label(npc_id);
                                    if ui
                                        .selectable_label(
                                            self.selected_npc_id == Some(npc_id),
                                            label,
                                        )
                                        .clicked()
                                    {
                                        self.selected_npc_id = Some(npc_id);
                                        self.active_tab_index =
                                            first_available_tab_index(&self.list_npc, npc_id)
                                                .unwrap_or(0);
                                    }
                                }
                            });
                    },
                );
            }
            SelectionMode::Zone => {
                ui.label("Filter Zone");
                ui.text_edit_singleline(&mut self.zone_filter);
                let filter = self.zone_filter.to_lowercase();
                let filtered_zones = self
                    .zone_ids
                    .iter()
                    .copied()
                    .filter(|zone_id| zone_matches_filter(*zone_id, &filter, &self.zone_list))
                    .collect::<Vec<_>>();

                let zone_list_height = (ui.available_height() * 0.45).clamp(180.0, 320.0);

                ui.label(RichText::new("Zones").strong());
                ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), zone_list_height),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ScrollArea::vertical()
                            .id_source("zone_list_scroll")
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                for zone_id in filtered_zones {
                                    if ui
                                        .selectable_label(
                                            self.selected_zone_id == Some(zone_id),
                                            self.zone_label(zone_id),
                                        )
                                        .clicked()
                                    {
                                        self.selected_zone_id = Some(zone_id);
                                        self.selected_npc_id =
                                            self.zone_shopkeepers.get(&zone_id.get()).and_then(
                                                |entries| entries.first().map(|entry| entry.npc_id),
                                            );
                                        if let Some(npc_id) = self.selected_npc_id {
                                            self.active_tab_index =
                                                first_available_tab_index(&self.list_npc, npc_id)
                                                    .unwrap_or(0);
                                        }
                                    }
                                }
                            });
                    },
                );

                ui.add_space(8.0);
                ui.separator();
                ui.label(RichText::new("Zone Shopkeepers").strong());
                let zone_entries = self
                    .selected_zone_id
                    .and_then(|zone_id| self.zone_shopkeepers.get(&zone_id.get()))
                    .cloned()
                    .unwrap_or_default();

                let shopkeeper_height = ui.available_height().max(140.0);
                ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), shopkeeper_height),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ScrollArea::vertical()
                            .id_source("zone_shopkeepers_scroll")
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                for entry in zone_entries {
                                    let label = format!(
                                        "{} ({})",
                                        self.npc_label(entry.npc_id),
                                        entry.spawn_count
                                    );
                                    if ui
                                        .selectable_label(
                                            self.selected_npc_id == Some(entry.npc_id),
                                            label,
                                        )
                                        .clicked()
                                    {
                                        self.selected_npc_id = Some(entry.npc_id);
                                        self.active_tab_index =
                                            first_available_tab_index(&self.list_npc, entry.npc_id)
                                                .unwrap_or(0);
                                    }
                                }
                            });
                    },
                );
            }
        }
    }

    fn draw_shop_editor(&mut self, ui: &mut egui::Ui) {
        let Some(selected_npc_id) = self.selected_npc_id else {
            ui.heading("Select a shopkeeper to edit.");
            return;
        };

        let npc_name = self
            .npc_database
            .get_npc(rose_data::NpcId::new(selected_npc_id).unwrap())
            .map(|npc| npc.name)
            .unwrap_or("Unknown NPC");
        ui.heading(format!("{} ({})", npc_name, selected_npc_id));
        ui.label("Edits apply to the selected NPC template and all of its spawns.");
        ui.separator();

        let tabs = npc_tab_ids(&self.list_npc, selected_npc_id);
        if tabs.iter().all(Option::is_none) {
            ui.label("This NPC does not expose any editable shop tabs.");
            return;
        }

        if tabs[self.active_tab_index].is_none() {
            self.active_tab_index = tabs.iter().position(Option::is_some).unwrap_or(0);
        }

        ui.horizontal(|ui| {
            for (tab_index, tab_id) in tabs.into_iter().enumerate() {
                match tab_id {
                    Some(tab_id) => {
                        let title = self.tab_label(tab_id);
                        if ui
                            .selectable_label(self.active_tab_index == tab_index, title)
                            .clicked()
                        {
                            self.active_tab_index = tab_index;
                        }
                    }
                    None => {
                        ui.add_enabled(false, Button::new(format!("Tab {}", tab_index + 1)));
                    }
                }
            }
        });

        if let Some(tab_id) = npc_tab_ids(&self.list_npc, selected_npc_id)[self.active_tab_index] {
            let shared_count = self
                .tab_usage
                .get(&tab_id)
                .map(|npcs| npcs.len())
                .unwrap_or(0);
            if shared_count > 1 {
                ui.label(format!(
                    "This tab is shared by {} NPC templates. The first edit will clone it for this NPC only.",
                    shared_count
                ));
            }

            ui.separator();
            let items = tab_items(&self.list_sell, tab_id);
            if items.is_empty() {
                ui.label("No items in this tab.");
            } else {
                ScrollArea::vertical().show(ui, |ui| {
                    egui::Grid::new("shop_items_grid")
                        .striped(true)
                        .num_columns(4)
                        .show(ui, |ui| {
                            ui.strong("Slot");
                            ui.strong("Item");
                            ui.strong("Category");
                            ui.strong("Action");
                            ui.end_row();

                            for (slot_index, item_reference) in items.iter().enumerate() {
                                ui.label(format!("{}", slot_index + 1));
                                self.draw_item_summary(ui, *item_reference, 28.0);
                                ui.label(self.item_category_label(*item_reference));
                                if ui.button("Remove").clicked() {
                                    if let Err(error) = self
                                        .remove_item_from_active_tab(selected_npc_id, slot_index)
                                    {
                                        self.status_message =
                                            format!("Failed to remove item: {error:#}");
                                    }
                                }
                                ui.end_row();
                            }
                        });
                });
            }
        }
    }

    fn draw_item_browser(&mut self, ui: &mut egui::Ui) {
        ui.heading("Item Browser");
        ui.label("Search by item id or name, then add into the active tab.");
        ui.separator();

        ui.label("Search");
        ui.text_edit_singleline(&mut self.item_filter);

        ComboBox::from_label("Item Type")
            .selected_text(
                self.item_type_filter
                    .map(item_type_label)
                    .unwrap_or("All types"),
            )
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut self.item_type_filter, None, "All types");
                for item_type in ITEM_TYPES {
                    ui.selectable_value(
                        &mut self.item_type_filter,
                        Some(item_type),
                        item_type_label(item_type),
                    );
                }
            });

        let available_classes = self.available_item_classes();
        if let Some(selected_class) = self.item_class_filter {
            if !available_classes.contains(&selected_class) {
                self.item_class_filter = None;
            }
        }

        ComboBox::from_label("Item Class")
            .selected_text(
                self.item_class_filter
                    .map(|item_class| self.item_class_label(item_class))
                    .unwrap_or_else(|| String::from("All classes")),
            )
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut self.item_class_filter, None, "All classes");
                for item_class in available_classes {
                    let label = self.item_class_label(item_class);
                    ui.selectable_value(&mut self.item_class_filter, Some(item_class), label);
                }
            });

        ui.separator();

        let filtered_items = self.filtered_items();
        ScrollArea::vertical().show(ui, |ui| {
            for item_reference in filtered_items {
                ui.horizontal(|ui| {
                    if ui.button("Add").clicked() {
                        if let Some(selected_npc_id) = self.selected_npc_id {
                            if let Err(error) =
                                self.add_item_to_active_tab(selected_npc_id, item_reference)
                            {
                                self.status_message = format!("Failed to add item: {error:#}");
                            }
                        } else {
                            self.status_message =
                                String::from("Select an NPC before adding items.");
                        }
                    }
                    self.draw_item_summary(ui, item_reference, 28.0);
                });
                ui.label(self.item_category_label(item_reference));
                ui.separator();
            }
        });
    }

    fn draw_item_summary(&self, ui: &mut egui::Ui, item_reference: ItemReference, size: f32) {
        let item_name = self.item_name(item_reference);
        let item_code = format!(
            "[{}:{}]",
            item_type_label(item_reference.item_type),
            item_reference.item_number
        );

        ui.horizontal(|ui| {
            if let Some(icon) = self.item_icon(item_reference) {
                ui.add(egui::Image::new((icon.texture_id, egui::vec2(size, size))).uv(icon.uv));
            } else {
                ui.allocate_space(egui::vec2(size, size));
            }

            ui.add(egui::Label::new(format!("{item_name} {item_code}")).wrap(false));
        });
    }

    fn item_name(&self, item_reference: ItemReference) -> String {
        self.item_database
            .get_base_item(item_reference)
            .map(|item| item.name.to_string())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| String::from("Unknown item"))
    }

    fn item_label(&self, item_reference: ItemReference) -> String {
        self.item_database
            .get_base_item(item_reference)
            .map(|item| {
                format!(
                    "[{}:{}] {}",
                    item_type_label(item_reference.item_type),
                    item_reference.item_number,
                    item.name
                )
            })
            .unwrap_or_else(|| {
                format!(
                    "[{}:{}] Unknown item",
                    item_type_label(item_reference.item_type),
                    item_reference.item_number
                )
            })
    }

    fn item_category_label(&self, item_reference: ItemReference) -> String {
        self.item_database
            .get_base_item(item_reference)
            .map(|item| {
                format!(
                    "{} / {}",
                    item_type_label(item_reference.item_type),
                    self.item_class_label(item.class)
                )
            })
            .unwrap_or_else(|| item_type_label(item_reference.item_type).to_string())
    }

    fn item_class_label(&self, item_class: ItemClass) -> String {
        let label = self.string_database.get_item_class(item_class);
        if label.is_empty() {
            format!("{item_class:?}")
        } else {
            label.to_string()
        }
    }

    fn item_icon(&self, item_reference: ItemReference) -> Option<ItemIcon> {
        let item_data = self.item_database.get_base_item(item_reference)?;
        let icons = self.item_icons.as_ref()?;
        let sprite = icons.sprites.get(item_data.icon_index as usize)?;
        let texture = icons.textures.get(sprite.texture_id as usize)?;

        Some(ItemIcon {
            texture_id: texture.handle.id(),
            uv: egui::Rect::from_min_max(
                egui::pos2(
                    (sprite.left as f32 + 0.5) / texture.size.x,
                    (sprite.top as f32 + 0.5) / texture.size.y,
                ),
                egui::pos2(
                    (sprite.right as f32 + 1.0) / texture.size.x,
                    (sprite.bottom as f32 + 1.0) / texture.size.y,
                ),
            ),
        })
    }

    fn npc_label(&self, npc_id: u16) -> String {
        let name = self
            .npc_database
            .get_npc(rose_data::NpcId::new(npc_id).unwrap())
            .map(|npc| npc.name)
            .filter(|name| !name.is_empty())
            .unwrap_or("Unnamed NPC");
        format!("[{}] {}", npc_id, name)
    }

    fn zone_label(&self, zone_id: ZoneId) -> String {
        let name = self
            .zone_list
            .get_zone(zone_id)
            .map(|zone| zone.name)
            .filter(|name| !name.is_empty())
            .unwrap_or("Unnamed Zone");
        format!("[{}] {}", zone_id.get(), name)
    }

    fn tab_label(&self, tab_id: u16) -> String {
        let key = self.list_sell.get(tab_id as usize, 1);
        if key.is_empty() {
            return format!("Tab {}", tab_id);
        }

        self.string_database
            .get_npc_store_tab(key)
            .map(|entry| entry.text.to_string())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| key.to_string())
    }

    fn available_item_classes(&self) -> Vec<ItemClass> {
        let mut classes = Vec::new();
        for entry in &self.item_catalog {
            if self
                .item_type_filter
                .map_or(true, |item_type| entry.reference.item_type == item_type)
                && !classes.contains(&entry.class)
            {
                classes.push(entry.class);
            }
        }
        classes
    }

    fn filtered_items(&self) -> Vec<ItemReference> {
        let filter = self.item_filter.to_lowercase();
        self.item_catalog
            .iter()
            .filter(|entry| {
                self.item_type_filter
                    .map_or(true, |item_type| entry.reference.item_type == item_type)
                    && self
                        .item_class_filter
                        .map_or(true, |item_class| entry.class == item_class)
                    && item_matches_filter(entry.reference, &filter, &self.item_database)
            })
            .map(|entry| entry.reference)
            .collect()
    }

    fn add_item_to_active_tab(
        &mut self,
        npc_id: u16,
        item_reference: ItemReference,
    ) -> Result<(), anyhow::Error> {
        let tab_id = ensure_unique_tab_for_npc(
            &mut self.list_npc,
            &mut self.list_sell,
            &mut self.tab_usage,
            npc_id,
            self.active_tab_index,
        )?;
        let mut items = tab_items(&self.list_sell, tab_id);
        if items.len() >= SHOP_ITEM_SLOT_COUNT {
            return Err(anyhow!("The selected tab is already full"));
        }

        items.push(item_reference);
        replace_tab_items(&mut self.list_sell, tab_id, &items)?;
        self.dirty = true;
        self.status_message = format!(
            "Added {} to NPC {}",
            self.item_label(item_reference),
            npc_id
        );
        Ok(())
    }

    fn remove_item_from_active_tab(
        &mut self,
        npc_id: u16,
        slot_index: usize,
    ) -> Result<(), anyhow::Error> {
        let tab_id = ensure_unique_tab_for_npc(
            &mut self.list_npc,
            &mut self.list_sell,
            &mut self.tab_usage,
            npc_id,
            self.active_tab_index,
        )?;
        let mut items = tab_items(&self.list_sell, tab_id);
        if slot_index >= items.len() {
            return Err(anyhow!("Invalid tab slot {}", slot_index));
        }

        let removed_item = items.remove(slot_index);
        replace_tab_items(&mut self.list_sell, tab_id, &items)?;
        self.dirty = true;
        self.status_message = format!(
            "Removed {} from NPC {}",
            self.item_label(removed_item),
            npc_id
        );
        Ok(())
    }

    fn save(&mut self) -> Result<(), anyhow::Error> {
        let mut npc_writer = RoseFileWriter::default();
        self.list_npc.write(&mut npc_writer, &())?;

        let mut sell_writer = RoseFileWriter::default();
        self.list_sell.write(&mut sell_writer, &())?;

        let replacements = HashMap::from([
            (VfsPathBuf::new(LIST_NPC_PATH), npc_writer.buffer.to_vec()),
            (VfsPathBuf::new(LIST_SELL_PATH), sell_writer.buffer.to_vec()),
        ]);

        let backup_count = match &mut self.save_target {
            SaveTarget::Packed {
                data_idx_path,
                writer_index,
            } => {
                let result = writer_index.rewrite_files(data_idx_path, &replacements)?;
                *writer_index = VfsIndex::load(data_idx_path)?;
                result.backups.len()
            }
            SaveTarget::Extracted { root_path } => {
                save_extracted_files(root_path, &replacements)?.len()
            }
        };
        self.dirty = false;
        self.status_message = format!("Saved shop data. Created {} backup file(s).", backup_count);
        Ok(())
    }
}

fn load_item_icons(
    vfs: &VirtualFilesystem,
    egui_ctx: &egui::Context,
) -> Result<ItemIconAtlas, anyhow::Error> {
    let tsi = vfs.read_file::<TsiFile, _>("3DDATA/CONTROL/RES/ITEM1.TSI")?;
    let mut textures = Vec::with_capacity(tsi.textures.len());

    for (index, texture) in tsi.textures.iter().enumerate() {
        let vfs_path = format!("3DDATA/CONTROL/RES/{}", texture.filename);
        let bytes = match vfs.open_file(&vfs_path) {
            Ok(file) => match file {
                rose_file_readers::VfsFile::Buffer(buffer) => buffer,
                rose_file_readers::VfsFile::View(view) => view.to_vec(),
            },
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("Failed to open item icon texture {}", vfs_path));
            }
        };

        let color_image = decode_texture_to_color_image(&texture.filename, &bytes)
            .with_context(|| format!("Failed to decode {}", texture.filename))?;
        let [width, height] = color_image.size;
        let handle = egui_ctx.load_texture(
            format!("shop_editor_item_sheet_{}", index),
            color_image,
            egui::TextureOptions::NEAREST,
        );

        textures.push(ItemIconTexture {
            handle,
            size: egui::vec2(width as f32, height as f32),
        });
    }

    Ok(ItemIconAtlas {
        sprites: tsi.sprites,
        textures,
    })
}

fn decode_texture_to_color_image(
    filename: &str,
    bytes: &[u8],
) -> Result<ColorImage, anyhow::Error> {
    let extension = Path::new(filename)
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase());

    if matches!(extension.as_deref(), Some("dds")) {
        return decode_dds_to_color_image(bytes);
    }

    let format = image_format_for_filename(filename, bytes)?;
    let image = image::load_from_memory_with_format(bytes, format)?.to_rgba8();
    let [width, height] = [image.width() as usize, image.height() as usize];
    Ok(ColorImage::from_rgba_unmultiplied(
        [width, height],
        image.as_raw(),
    ))
}

fn decode_dds_to_color_image(bytes: &[u8]) -> Result<ColorImage, anyhow::Error> {
    let dds = Dds::read(&mut Cursor::new(bytes)).context("Failed to parse DDS")?;
    let width = dds.get_width() as usize;
    let height = dds.get_height() as usize;
    if width == 0 || height == 0 {
        return Err(anyhow!("DDS texture has invalid dimensions"));
    }

    let format = detect_dds_format(&dds).map_err(|error| anyhow!(error))?;
    let mip0 = dds
        .get_data(0)
        .map_err(|error| anyhow!("Failed to read DDS base mip: {}", error))?;
    let mip0_size = dds_level_size(width, height, format).map_err(|error| anyhow!(error))?;
    if mip0.len() < mip0_size {
        return Err(anyhow!(
            "DDS base mip too small: expected at least {}, got {}",
            mip0_size,
            mip0.len()
        ));
    }

    let rgba = decode_dds_rgba(&mip0[..mip0_size], width, height, format)
        .map_err(|error| anyhow!(error))?;
    Ok(ColorImage::from_rgba_unmultiplied([width, height], &rgba))
}

fn detect_dds_format(dds: &Dds) -> Result<DdsTextureFormat, String> {
    if let Some(d3d_format) = dds.get_d3d_format() {
        return match d3d_format {
            D3DFormat::DXT1 => Ok(DdsTextureFormat::Compressed(BcFormat::Bc1)),
            D3DFormat::DXT3 => Ok(DdsTextureFormat::Compressed(BcFormat::Bc2)),
            D3DFormat::DXT5 => Ok(DdsTextureFormat::Compressed(BcFormat::Bc3)),
            D3DFormat::A8R8G8B8 => Ok(DdsTextureFormat::A8R8G8B8),
            D3DFormat::X8R8G8B8 => Ok(DdsTextureFormat::X8R8G8B8),
            D3DFormat::R8G8B8 => Ok(DdsTextureFormat::R8G8B8),
            D3DFormat::A4R4G4B4 => Ok(DdsTextureFormat::A4R4G4B4),
            D3DFormat::R5G6B5 => Ok(DdsTextureFormat::R5G6B5),
            D3DFormat::A1R5G5B5 => Ok(DdsTextureFormat::A1R5G5B5),
            other => Err(format!("Unsupported D3D DDS format: {:?}", other)),
        };
    }

    if let Some(dxgi_format) = dds.get_dxgi_format() {
        return match dxgi_format {
            DxgiFormat::BC1_UNorm | DxgiFormat::BC1_UNorm_sRGB => {
                Ok(DdsTextureFormat::Compressed(BcFormat::Bc1))
            }
            DxgiFormat::BC2_UNorm | DxgiFormat::BC2_UNorm_sRGB => {
                Ok(DdsTextureFormat::Compressed(BcFormat::Bc2))
            }
            DxgiFormat::BC3_UNorm | DxgiFormat::BC3_UNorm_sRGB => {
                Ok(DdsTextureFormat::Compressed(BcFormat::Bc3))
            }
            DxgiFormat::B8G8R8A8_UNorm | DxgiFormat::B8G8R8A8_UNorm_sRGB => {
                Ok(DdsTextureFormat::A8R8G8B8)
            }
            DxgiFormat::B8G8R8X8_UNorm | DxgiFormat::B8G8R8X8_UNorm_sRGB => {
                Ok(DdsTextureFormat::X8R8G8B8)
            }
            DxgiFormat::B5G6R5_UNorm => Ok(DdsTextureFormat::R5G6B5),
            DxgiFormat::B5G5R5A1_UNorm => Ok(DdsTextureFormat::A1R5G5B5),
            DxgiFormat::B4G4R4A4_UNorm => Ok(DdsTextureFormat::A4R4G4B4),
            other => Err(format!("Unsupported DXGI DDS format: {:?}", other)),
        };
    }

    Err(String::from("Could not determine DDS pixel format"))
}

fn dds_level_size(width: usize, height: usize, format: DdsTextureFormat) -> Result<usize, String> {
    let size = match format {
        DdsTextureFormat::Compressed(bc_format) => {
            let block_count_x = (width + 3) / 4;
            let block_count_y = (height + 3) / 4;
            let bytes_per_block = match bc_format {
                BcFormat::Bc1 => 8usize,
                BcFormat::Bc2 | BcFormat::Bc3 => 16usize,
                other => return Err(format!("Unsupported BC format for DDS sizing: {:?}", other)),
            };
            block_count_x
                .checked_mul(block_count_y)
                .and_then(|n| n.checked_mul(bytes_per_block))
        }
        DdsTextureFormat::A8R8G8B8 | DdsTextureFormat::X8R8G8B8 => {
            width.checked_mul(height).and_then(|n| n.checked_mul(4))
        }
        DdsTextureFormat::R8G8B8 => width.checked_mul(height).and_then(|n| n.checked_mul(3)),
        DdsTextureFormat::A4R4G4B4 | DdsTextureFormat::R5G6B5 | DdsTextureFormat::A1R5G5B5 => {
            width.checked_mul(height).and_then(|n| n.checked_mul(2))
        }
    };

    size.ok_or_else(|| String::from("DDS mip size overflow"))
}

fn decode_dds_rgba(
    data: &[u8],
    width: usize,
    height: usize,
    format: DdsTextureFormat,
) -> Result<Vec<u8>, String> {
    match format {
        DdsTextureFormat::Compressed(bc_format) => {
            let mut output = vec![0u8; width * height * 4];
            bc_format.decompress(data, width, height, &mut output);
            Ok(output)
        }
        DdsTextureFormat::A8R8G8B8 => {
            let expected = width * height * 4;
            if data.len() < expected {
                return Err(format!(
                    "A8R8G8B8 source too small: expected at least {}, got {}",
                    expected,
                    data.len()
                ));
            }

            let mut output = vec![0u8; width * height * 4];
            for i in 0..(width * height) {
                let si = i * 4;
                output[si] = data[si + 2];
                output[si + 1] = data[si + 1];
                output[si + 2] = data[si];
                output[si + 3] = data[si + 3];
            }
            Ok(output)
        }
        DdsTextureFormat::X8R8G8B8 => {
            let expected = width * height * 4;
            if data.len() < expected {
                return Err(format!(
                    "X8R8G8B8 source too small: expected at least {}, got {}",
                    expected,
                    data.len()
                ));
            }

            let mut output = vec![0u8; width * height * 4];
            for i in 0..(width * height) {
                let si = i * 4;
                output[si] = data[si + 2];
                output[si + 1] = data[si + 1];
                output[si + 2] = data[si];
                output[si + 3] = 255;
            }
            Ok(output)
        }
        DdsTextureFormat::R8G8B8 => {
            let expected = width * height * 3;
            if data.len() < expected {
                return Err(format!(
                    "R8G8B8 source too small: expected at least {}, got {}",
                    expected,
                    data.len()
                ));
            }

            let mut output = vec![0u8; width * height * 4];
            for i in 0..(width * height) {
                let si = i * 3;
                let di = i * 4;
                output[di] = data[si + 2];
                output[di + 1] = data[si + 1];
                output[di + 2] = data[si];
                output[di + 3] = 255;
            }
            Ok(output)
        }
        DdsTextureFormat::A4R4G4B4 => {
            let expected = width * height * 2;
            if data.len() < expected {
                return Err(format!(
                    "A4R4G4B4 source too small: expected at least {}, got {}",
                    expected,
                    data.len()
                ));
            }

            let mut output = vec![0u8; width * height * 4];
            for i in 0..(width * height) {
                let si = i * 2;
                let pixel = (data[si] as u16) | ((data[si + 1] as u16) << 8);
                let b = ((pixel & 0x000F) * 255 / 15) as u8;
                let g = (((pixel >> 4) & 0x000F) * 255 / 15) as u8;
                let r = (((pixel >> 8) & 0x000F) * 255 / 15) as u8;
                let a = (((pixel >> 12) & 0x000F) * 255 / 15) as u8;
                let di = i * 4;
                output[di] = r;
                output[di + 1] = g;
                output[di + 2] = b;
                output[di + 3] = a;
            }
            Ok(output)
        }
        DdsTextureFormat::R5G6B5 => {
            let expected = width * height * 2;
            if data.len() < expected {
                return Err(format!(
                    "R5G6B5 source too small: expected at least {}, got {}",
                    expected,
                    data.len()
                ));
            }

            let mut output = vec![0u8; width * height * 4];
            for i in 0..(width * height) {
                let si = i * 2;
                let pixel = (data[si] as u16) | ((data[si + 1] as u16) << 8);
                let b = ((pixel & 0x001F) * 255 / 31) as u8;
                let g = (((pixel >> 5) & 0x003F) * 255 / 63) as u8;
                let r = (((pixel >> 11) & 0x001F) * 255 / 31) as u8;
                let di = i * 4;
                output[di] = r;
                output[di + 1] = g;
                output[di + 2] = b;
                output[di + 3] = 255;
            }
            Ok(output)
        }
        DdsTextureFormat::A1R5G5B5 => {
            let expected = width * height * 2;
            if data.len() < expected {
                return Err(format!(
                    "A1R5G5B5 source too small: expected at least {}, got {}",
                    expected,
                    data.len()
                ));
            }

            let mut output = vec![0u8; width * height * 4];
            for i in 0..(width * height) {
                let si = i * 2;
                let pixel = (data[si] as u16) | ((data[si + 1] as u16) << 8);
                let b = ((pixel & 0x001F) * 255 / 31) as u8;
                let g = (((pixel >> 5) & 0x001F) * 255 / 31) as u8;
                let r = (((pixel >> 10) & 0x001F) * 255 / 31) as u8;
                let a = if pixel & 0x8000 != 0 { 255u8 } else { 0u8 };
                let di = i * 4;
                output[di] = r;
                output[di + 1] = g;
                output[di + 2] = b;
                output[di + 3] = a;
            }
            Ok(output)
        }
    }
}

fn image_format_for_filename(filename: &str, bytes: &[u8]) -> Result<ImageFormat, anyhow::Error> {
    let extension = Path::new(filename)
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase());

    match extension.as_deref() {
        Some("dds") => Ok(ImageFormat::Dds),
        Some("tga") => Ok(ImageFormat::Tga),
        Some("png") => Ok(ImageFormat::Png),
        Some("bmp") => Ok(ImageFormat::Bmp),
        _ => image::guess_format(bytes).map_err(|error| {
            anyhow!(
                "Unable to determine image format for {}: {}",
                filename,
                error
            )
        }),
    }
}

fn create_virtual_filesystem(
    resolved_input: &ResolvedInputSource,
) -> Result<VirtualFilesystem, anyhow::Error> {
    match &resolved_input.save_target {
        SaveTarget::Packed { data_idx_path, .. } => {
            let reader_index = VfsIndex::load(data_idx_path)
                .with_context(|| format!("Failed to load {}", data_idx_path.display()))?;
            Ok(VirtualFilesystem::new(vec![Box::new(reader_index)]))
        }
        SaveTarget::Extracted { root_path } => Ok(VirtualFilesystem::new(vec![Box::new(
            rose_file_readers::HostFilesystemDevice::new(root_path.clone()),
        )])),
    }
}

fn resolve_input_source(input_path: &Path) -> Result<ResolvedInputSource, anyhow::Error> {
    if input_path.is_file() {
        if input_path
            .file_name()
            .and_then(|name| name.to_str())
            .map(|name| name.eq_ignore_ascii_case("data.idx"))
            .unwrap_or(false)
        {
            return Ok(ResolvedInputSource {
                source_kind: SourceKind::PackedDataIdx,
                display_path: input_path.to_path_buf(),
                save_target: SaveTarget::Packed {
                    data_idx_path: input_path.to_path_buf(),
                    writer_index: VfsIndex::load(input_path)
                        .with_context(|| format!("Failed to load {}", input_path.display()))?,
                },
            });
        }

        return Err(anyhow!(
            "Input file must be a data.idx path, or choose an extracted data folder"
        ));
    }

    if !input_path.is_dir() {
        return Err(anyhow!("Could not find {}", input_path.display()));
    }

    let packed_candidate = input_path.join("data.idx");
    if packed_candidate.exists() {
        if input_path.join("data.prf").exists()
            || input_path.join("data.trf").exists()
            || input_path.join("data.rose").exists()
        {
            return Err(anyhow!(
                "Only the standard ROSE data.idx format is supported by this editor"
            ));
        }

        return Ok(ResolvedInputSource {
            source_kind: SourceKind::PackedDataIdx,
            display_path: packed_candidate.clone(),
            save_target: SaveTarget::Packed {
                data_idx_path: packed_candidate.clone(),
                writer_index: VfsIndex::load(&packed_candidate)
                    .with_context(|| format!("Failed to load {}", packed_candidate.display()))?,
            },
        });
    }

    if looks_like_extracted_root(input_path) {
        return Ok(ResolvedInputSource {
            source_kind: SourceKind::ExtractedData,
            display_path: input_path.join("3DDATA"),
            save_target: SaveTarget::Extracted {
                root_path: input_path.to_path_buf(),
            },
        });
    }

    if looks_like_3ddata_folder(input_path) {
        let root_path = input_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| input_path.to_path_buf());
        return Ok(ResolvedInputSource {
            source_kind: SourceKind::ExtractedData,
            display_path: input_path.to_path_buf(),
            save_target: SaveTarget::Extracted { root_path },
        });
    }

    Err(anyhow!(
        "Input must be a ROSE root folder with data.idx, a data.idx file, an extracted data root containing 3DDATA, or the extracted 3DDATA folder itself"
    ))
}

fn looks_like_extracted_root(path: &Path) -> bool {
    path.join(LIST_NPC_PATH).exists() && path.join(LIST_SELL_PATH).exists()
}

fn looks_like_3ddata_folder(path: &Path) -> bool {
    path.join("STB").join("LIST_NPC.STB").exists()
        && path.join("STB").join("LIST_SELL.STB").exists()
}

fn save_extracted_files(
    root_path: &Path,
    replacements: &HashMap<VfsPathBuf, Vec<u8>>,
) -> Result<Vec<PathBuf>, anyhow::Error> {
    let timestamp = backup_timestamp();
    let mut backups = Vec::new();

    for (vfs_path, bytes) in replacements {
        let target_path = root_path.join(vfs_path.path());
        if let Some(parent_path) = target_path.parent() {
            std::fs::create_dir_all(parent_path)
                .with_context(|| format!("Failed to create directory {}", parent_path.display()))?;
        }

        if target_path.exists() {
            backups.push(create_file_backup(&target_path, &timestamp)?);
        }

        let temp_path = extracted_temp_file_path(&target_path, &timestamp);
        std::fs::write(&temp_path, bytes)
            .with_context(|| format!("Failed to write {}", temp_path.display()))?;
        replace_extracted_file(&target_path, &temp_path)?;
    }

    Ok(backups)
}

fn backup_timestamp() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().to_string())
        .unwrap_or_else(|_| String::from("0"))
}

fn extracted_temp_file_path(path: &Path, timestamp: &str) -> PathBuf {
    let filename = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| String::from("temp"));
    path.with_file_name(format!("{}.{}.tmp", filename, timestamp))
}

fn create_file_backup(path: &Path, timestamp: &str) -> Result<PathBuf, anyhow::Error> {
    let filename = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| String::from("backup"));
    let backup_path = path.with_file_name(format!("{}.{}.bak", filename, timestamp));
    std::fs::copy(path, &backup_path).with_context(|| {
        format!(
            "Failed to create backup {} from {}",
            backup_path.display(),
            path.display()
        )
    })?;
    Ok(backup_path)
}

fn replace_extracted_file(target_path: &Path, temp_path: &Path) -> Result<(), anyhow::Error> {
    if target_path.exists() {
        std::fs::remove_file(target_path)
            .with_context(|| format!("Failed to remove {}", target_path.display()))?;
    }
    std::fs::rename(temp_path, target_path).with_context(|| {
        format!(
            "Failed to move {} into place at {}",
            temp_path.display(),
            target_path.display()
        )
    })?;
    Ok(())
}

fn build_zone_shopkeepers(
    zone_database: &ZoneDatabase,
    list_npc: &StbFile,
    list_sell: &StbFile,
) -> HashMap<u16, Vec<ZoneNpcSummary>> {
    let mut result = HashMap::new();

    for zone in zone_database.iter() {
        let mut counts = HashMap::<u16, usize>::new();
        for spawn in &zone.npcs {
            let npc_id = spawn.npc_id.get();
            if npc_sells_items(list_npc, list_sell, npc_id) {
                *counts.entry(npc_id).or_default() += 1;
            }
        }

        let mut entries = counts
            .into_iter()
            .map(|(npc_id, spawn_count)| ZoneNpcSummary {
                npc_id,
                spawn_count,
            })
            .collect::<Vec<_>>();
        entries.sort_by_key(|entry| entry.npc_id);
        if !entries.is_empty() {
            result.insert(zone.id.get(), entries);
        }
    }

    result
}

fn build_item_catalog(item_database: &ItemDatabase) -> Vec<ItemCatalogEntry> {
    let mut items = Vec::new();
    for item_type in ITEM_TYPES {
        for item_reference in item_database.iter_items(item_type) {
            if let Some(base_item) = item_database.get_base_item(item_reference) {
                if !base_item.name.is_empty() {
                    items.push(ItemCatalogEntry {
                        reference: item_reference,
                        class: base_item.class,
                    });
                }
            }
        }
    }
    items
}

fn build_tab_usage(list_npc: &StbFile) -> HashMap<u16, Vec<u16>> {
    let mut usage = HashMap::<u16, Vec<u16>>::new();
    for row in 0..list_npc.rows() {
        let npc_id = row as u16;
        for tab_id in npc_tab_ids(list_npc, npc_id).into_iter().flatten() {
            let entry = usage.entry(tab_id).or_default();
            if !entry.contains(&npc_id) {
                entry.push(npc_id);
            }
        }
    }
    usage
}

fn npc_sells_items(list_npc: &StbFile, list_sell: &StbFile, npc_id: u16) -> bool {
    npc_tab_ids(list_npc, npc_id)
        .into_iter()
        .flatten()
        .any(|tab_id| !tab_items(list_sell, tab_id).is_empty())
}

fn npc_tab_ids(list_npc: &StbFile, npc_id: u16) -> [Option<u16>; NPC_STORE_TAB_COUNT] {
    let mut tabs = [None; NPC_STORE_TAB_COUNT];
    for (offset, tab) in tabs.iter_mut().enumerate() {
        let value = list_npc.get_int(npc_id as usize, NPC_STORE_TAB_COLUMN_START + offset);
        if value > 0 {
            *tab = Some(value as u16);
        }
    }
    tabs
}

fn first_available_tab_index(list_npc: &StbFile, npc_id: u16) -> Option<usize> {
    npc_tab_ids(list_npc, npc_id)
        .iter()
        .position(Option::is_some)
}

fn tab_items(list_sell: &StbFile, tab_id: u16) -> Vec<ItemReference> {
    let mut items = Vec::new();
    for column in SHOP_ITEM_COLUMN_START..SHOP_ITEM_COLUMN_START + SHOP_ITEM_SLOT_COUNT {
        if let Some(item_reference) =
            decode_item_base1000(list_sell.get_int(tab_id as usize, column) as usize)
        {
            items.push(item_reference);
        }
    }
    items
}

fn replace_tab_items(
    list_sell: &mut StbFile,
    tab_id: u16,
    items: &[ItemReference],
) -> Result<(), anyhow::Error> {
    if items.len() > SHOP_ITEM_SLOT_COUNT {
        return Err(anyhow!(
            "Cannot store more than {} items per tab",
            SHOP_ITEM_SLOT_COUNT
        ));
    }

    for slot_index in 0..SHOP_ITEM_SLOT_COUNT {
        let cell_value = items
            .get(slot_index)
            .and_then(|item_reference| encode_item_base1000(*item_reference))
            .map(|value| value.to_string())
            .unwrap_or_default();
        list_sell.set(
            tab_id as usize,
            SHOP_ITEM_COLUMN_START + slot_index,
            cell_value,
        )?;
    }

    Ok(())
}

fn ensure_unique_tab_for_npc(
    list_npc: &mut StbFile,
    list_sell: &mut StbFile,
    tab_usage: &mut HashMap<u16, Vec<u16>>,
    npc_id: u16,
    tab_index: usize,
) -> Result<u16, anyhow::Error> {
    let existing_tab_id = npc_tab_ids(list_npc, npc_id)
        .get(tab_index)
        .and_then(|tab_id| *tab_id)
        .ok_or_else(|| {
            anyhow!(
                "NPC {} does not have a tab in slot {}",
                npc_id,
                tab_index + 1
            )
        })?;

    let shared_count = tab_usage
        .get(&existing_tab_id)
        .map(|npc_ids| npc_ids.len())
        .unwrap_or(0);
    if shared_count <= 1 {
        return Ok(existing_tab_id);
    }

    let new_tab_id = list_sell.rows();
    let row_values = (0..list_sell.columns())
        .map(|column| list_sell.get(existing_tab_id as usize, column).to_string())
        .collect::<Vec<_>>();
    list_sell.push_row(format!("SHOP_CLONE_{}", new_tab_id), row_values)?;
    list_npc.set(
        npc_id as usize,
        NPC_STORE_TAB_COLUMN_START + tab_index,
        new_tab_id.to_string(),
    )?;

    if let Some(npc_ids) = tab_usage.get_mut(&existing_tab_id) {
        npc_ids.retain(|existing_npc_id| *existing_npc_id != npc_id);
    }
    tab_usage.entry(new_tab_id as u16).or_default().push(npc_id);

    Ok(new_tab_id as u16)
}

fn encode_item_base1000(item_reference: ItemReference) -> Option<usize> {
    Some(encode_item_type(item_reference.item_type)? * 1000 + item_reference.item_number)
}

fn item_matches_filter(
    item_reference: ItemReference,
    filter: &str,
    item_database: &ItemDatabase,
) -> bool {
    if filter.is_empty() {
        return true;
    }

    if item_reference.item_number.to_string().contains(filter) {
        return true;
    }

    item_database
        .get_base_item(item_reference)
        .map(|item| item.name.to_lowercase().contains(filter))
        .unwrap_or(false)
}

fn npc_matches_filter(npc_id: u16, filter: &str, npc_database: &NpcDatabase) -> bool {
    if filter.is_empty() {
        return true;
    }

    if npc_id.to_string().contains(filter) {
        return true;
    }

    npc_database
        .get_npc(rose_data::NpcId::new(npc_id).unwrap())
        .map(|npc| npc.name.to_lowercase().contains(filter))
        .unwrap_or(false)
}

fn zone_matches_filter(zone_id: ZoneId, filter: &str, zone_list: &ZoneList) -> bool {
    if filter.is_empty() {
        return true;
    }

    if zone_id.get().to_string().contains(filter) {
        return true;
    }

    zone_list
        .get_zone(zone_id)
        .map(|zone| zone.name.to_lowercase().contains(filter))
        .unwrap_or(false)
}

fn item_type_label(item_type: ItemType) -> &'static str {
    match item_type {
        ItemType::Face => "Face",
        ItemType::Head => "Head",
        ItemType::Body => "Body",
        ItemType::Hands => "Hands",
        ItemType::Feet => "Feet",
        ItemType::Back => "Back",
        ItemType::Jewellery => "Jewellery",
        ItemType::Weapon => "Weapon",
        ItemType::SubWeapon => "SubWeapon",
        ItemType::Consumable => "Consumable",
        ItemType::Gem => "Gem",
        ItemType::Material => "Material",
        ItemType::Quest => "Quest",
        ItemType::Vehicle => "Vehicle",
    }
}

#[cfg(test)]
mod tests {
    use super::{
        build_tab_usage, ensure_unique_tab_for_npc, npc_sells_items, npc_tab_ids,
        replace_tab_items, tab_items, NPC_STORE_TAB_COLUMN_START, SHOP_ITEM_COLUMN_START,
    };
    use rose_data::{ItemReference, ItemType};
    use rose_file_readers::StbFile;

    fn build_list_npc() -> StbFile {
        let columns = (0..26).map(|index| format!("C{index}")).collect::<Vec<_>>();
        let mut cells = vec![String::new(); 3 * 25];
        cells[1 * 25 + NPC_STORE_TAB_COLUMN_START] = String::from("1");
        cells[2 * 25 + NPC_STORE_TAB_COLUMN_START] = String::from("1");
        StbFile::new(
            1,
            16,
            columns,
            "#",
            vec![
                String::from("NPC0"),
                String::from("NPC1"),
                String::from("NPC2"),
            ],
            cells,
        )
        .unwrap()
    }

    fn build_list_sell() -> StbFile {
        let columns = (0..51).map(|index| format!("C{index}")).collect::<Vec<_>>();
        let mut cells = vec![String::new(); 2 * 50];
        cells[1 * 50 + 1] = String::from("SELL_TAB");
        cells[1 * 50 + SHOP_ITEM_COLUMN_START] = String::from("1001");
        cells[1 * 50 + SHOP_ITEM_COLUMN_START + 1] = String::from("2003");
        StbFile::new(
            1,
            16,
            columns,
            "#",
            vec![String::from("EMPTY"), String::from("SELL1")],
            cells,
        )
        .unwrap()
    }

    #[test]
    fn shared_tab_is_cloned_before_editing() {
        let mut list_npc = build_list_npc();
        let mut list_sell = build_list_sell();
        let mut usage = build_tab_usage(&list_npc);

        let cloned_tab_id =
            ensure_unique_tab_for_npc(&mut list_npc, &mut list_sell, &mut usage, 1, 0).unwrap();

        assert_eq!(cloned_tab_id, 2);
        assert_eq!(npc_tab_ids(&list_npc, 1)[0], Some(2));
        assert_eq!(npc_tab_ids(&list_npc, 2)[0], Some(1));
        assert_eq!(list_sell.get_row_name(2), "SHOP_CLONE_2");
        assert_eq!(list_sell.get(2, 1), "SELL_TAB");
    }

    #[test]
    fn tab_item_updates_compact_slots() {
        let mut list_sell = build_list_sell();
        replace_tab_items(
            &mut list_sell,
            1,
            &[
                ItemReference::new(ItemType::Face, 7),
                ItemReference::new(ItemType::Head, 3),
            ],
        )
        .unwrap();

        let items = tab_items(&list_sell, 1);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0], ItemReference::new(ItemType::Face, 7));
        assert_eq!(items[1], ItemReference::new(ItemType::Head, 3));
        assert_eq!(list_sell.get_int(1, SHOP_ITEM_COLUMN_START + 2), 0);
    }

    #[test]
    fn npc_list_filter_requires_real_sell_items() {
        let list_npc = build_list_npc();
        let mut list_sell = build_list_sell();

        assert!(npc_sells_items(&list_npc, &list_sell, 1));

        replace_tab_items(&mut list_sell, 1, &[]).unwrap();

        assert!(!npc_sells_items(&list_npc, &list_sell, 1));
        assert!(!npc_sells_items(&list_npc, &list_sell, 2));
    }
}
