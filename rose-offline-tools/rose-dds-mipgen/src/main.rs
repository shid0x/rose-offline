use std::{
    collections::{HashMap, HashSet},
    f32::consts::PI,
    fs,
    io::Cursor,
    path::{Path, PathBuf},
};

use clap::Command;
use ddsfile::{D3DFormat, Dds, DxgiFormat};
use log::{error, info, warn};
use rose_file_readers::{
    AruaVfsIndex, HostFilesystemDevice, IrosePhVfsIndex, TitanVfsIndex, VfsFile, VfsIndex,
    VfsPathBuf, VirtualFilesystem, VirtualFilesystemDevice, ZscFile,
};
use serde::{Deserialize, Serialize};
use texpresso::Format;

mod discovery;

use discovery::{discover_textures, DiscoveryScope};

const DEFAULT_ALPHA_CUTOFF: f32 = 0.5;
const KAISER_RADIUS: f32 = 3.0;
const KAISER_BETA: f32 = 5.0;
const BUILTIN_EXCLUDED_TEXTURES: &[&str] = &[
    // Known problematic asset: mip generation introduces visible face artifacts at distance.
    "3DDATA/NPC/NPC/FAIRY/HEAD01.DDS",
    // Known problematic asset: mip generation introduces black artifacts on the bee body.
    "3DDATA/NPC/ANIMAL/BEE/BODY01_010.DDS",
    // Known problematic asset: mip generation introduces black artifacts on the raccoon head.
    "3DDATA/NPC/ANIMAL/RACCOON/HEAD01_010.DDS",
];

#[derive(Debug, Default, Clone, Copy, Serialize)]
struct TextureUsage {
    specular: bool,
    cutout: bool,
    regular_alpha: bool,
    alpha_cutoff: Option<f32>,
    overridden: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
enum FilterKind {
    Box,
    Lanczos3,
    Kaiser,
}

impl FilterKind {
    fn from_str(value: &str) -> Self {
        match value {
            "box" => Self::Box,
            "lanczos3" => Self::Lanczos3,
            "kaiser" => Self::Kaiser,
            _ => Self::Kaiser,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Box => "box",
            Self::Lanczos3 => "lanczos3",
            Self::Kaiser => "kaiser",
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum AlphaHandling {
    Regular,
    Specular,
    Cutout(f32),
}

#[derive(Debug, Deserialize)]
struct TextureOverrideFile {
    #[serde(default)]
    texture: Vec<TextureOverrideEntry>,
}

#[derive(Debug, Deserialize)]
struct TextureOverrideEntry {
    path: String,
    usage: TextureOverrideUsage,
    alpha_cutoff: Option<f32>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
enum TextureOverrideUsage {
    Specular,
    Cutout,
    Regular,
}

#[derive(Debug, Serialize)]
struct MipgenReport {
    discovery_scope: String,
    filter: String,
    textures_discovered: usize,
    specular_textures: usize,
    cutout_textures: usize,
    regular_textures: usize,
    generated_mipmaps: u32,
    skipped_existing: u32,
    skipped_excluded: u32,
    errors: u32,
    excluded_textures: Vec<String>,
    missing_textures: Vec<String>,
    unsupported_formats: Vec<String>,
    processing_errors: Vec<ProcessingError>,
    override_entries_applied: usize,
}

#[derive(Debug, Serialize)]
struct ProcessingError {
    path: String,
    stage: String,
    error: String,
}

#[derive(Clone, Copy, Debug)]
enum TexFormat {
    Compressed(Format),
    A8R8G8B8,
    X8R8G8B8,
    R8G8B8,
    A4R4G4B4,
    R5G6B5,
    A1R5G5B5,
}

impl TexFormat {
    fn has_alpha(self) -> bool {
        match self {
            TexFormat::Compressed(Format::Bc1) => false,
            TexFormat::Compressed(_) => true,
            TexFormat::A8R8G8B8 => true,
            TexFormat::X8R8G8B8 => false,
            TexFormat::R8G8B8 => false,
            TexFormat::A4R4G4B4 => true,
            TexFormat::R5G6B5 => false,
            TexFormat::A1R5G5B5 => true,
        }
    }
}

fn main() {
    let command = Command::new("rose-dds-mipgen")
        .about("Generate mipmaps for ROSE Online DDS textures")
        .long_about(
            "Discovers referenced DDS textures from game assets, classifies texture usage\n\
             (specular/cutout/regular), and generates mipmaps while preserving the\n\
             original base level to avoid recompression quality loss.",
        )
        .arg(
            clap::Arg::new("input-path")
                .long("input-path")
                .help("Directory of ROSE installation to read textures from.")
                .takes_value(true)
                .required(true),
        )
        .arg(
            clap::Arg::new("output-path")
                .long("output-path")
                .help("Directory to write mipmapped DDS files to.")
                .takes_value(true)
                .required(true),
        )
        .arg(
            clap::Arg::new("vfs-type")
                .long("vfs-type")
                .help("Which format to read the VFS as")
                .takes_value(true)
                .value_parser(["directory", "rose", "aruarose", "titanrose", "iroseph"])
                .default_value("directory"),
        )
        .arg(
            clap::Arg::new("dry-run")
                .long("dry-run")
                .help("Only scan and report what would be done, do not generate files."),
        )
        .arg(
            clap::Arg::new("filter")
                .long("filter")
                .help("Downsampling filter")
                .takes_value(true)
                .value_parser(["kaiser", "lanczos3", "box"])
                .default_value("kaiser"),
        )
        .arg(
            clap::Arg::new("discovery-scope")
                .long("discovery-scope")
                .help("Asset discovery strategy")
                .takes_value(true)
                .value_parser(["full", "zsc-only"])
                .default_value("full"),
        )
        .arg(
            clap::Arg::new("overrides")
                .long("overrides")
                .help("Path to TOML usage overrides file")
                .takes_value(true),
        )
        .arg(
            clap::Arg::new("report-json")
                .long("report-json")
                .help("Write run summary report as JSON")
                .takes_value(true),
        );

    let matches = command.get_matches();

    simplelog::TermLogger::init(
        simplelog::LevelFilter::Info,
        simplelog::Config::default(),
        simplelog::TerminalMode::Mixed,
        simplelog::ColorChoice::Auto,
    )
    .unwrap();

    let input_path = PathBuf::from(matches.value_of("input-path").unwrap());
    let output_path = PathBuf::from(matches.value_of("output-path").unwrap());
    let dry_run = matches.is_present("dry-run");

    let vfs_type = matches.value_of("vfs-type").unwrap_or("directory");
    let filter = FilterKind::from_str(matches.value_of("filter").unwrap_or("kaiser"));
    let discovery_scope =
        DiscoveryScope::from_str(matches.value_of("discovery-scope").unwrap_or("full"));
    let overrides_path = matches.value_of("overrides").map(PathBuf::from);
    let report_path = matches.value_of("report-json").map(PathBuf::from);

    let mut vfs_devices: Vec<Box<dyn VirtualFilesystemDevice + Send + Sync>> = Vec::new();
    match vfs_type {
        "directory" => {
            vfs_devices.push(Box::new(HostFilesystemDevice::new(input_path.clone())));
        }
        "rose" => {
            let index_root_path = input_path.parent().unwrap_or(Path::new("")).to_path_buf();
            vfs_devices.push(Box::new(
                VfsIndex::load(&input_path).expect("Failed to load VFS"),
            ));
            vfs_devices.push(Box::new(HostFilesystemDevice::new(index_root_path)));
        }
        "aruarose" => {
            let index_root_path = input_path.parent().unwrap_or(Path::new("")).to_path_buf();
            vfs_devices.push(Box::new(
                AruaVfsIndex::load(&input_path, &index_root_path.join("data.rose"))
                    .expect("Failed to load AruaVfs"),
            ));
            vfs_devices.push(Box::new(HostFilesystemDevice::new(index_root_path)));
        }
        "titanrose" => {
            let index_root_path = input_path.parent().unwrap_or(Path::new("")).to_path_buf();
            vfs_devices.push(Box::new(
                TitanVfsIndex::load(&input_path, &index_root_path.join("data.trf"))
                    .expect("Failed to load TitanVfs"),
            ));
            vfs_devices.push(Box::new(HostFilesystemDevice::new(index_root_path)));
        }
        "iroseph" => {
            let index_root_path = input_path.parent().unwrap_or(Path::new("")).to_path_buf();
            vfs_devices.push(Box::new(
                IrosePhVfsIndex::load(&input_path).expect("Failed to load iRosePH VFS"),
            ));
            vfs_devices.push(Box::new(HostFilesystemDevice::new(index_root_path)));
        }
        _ => panic!("Unknown VFS type: {}", vfs_type),
    }

    let vfs = VirtualFilesystem::new(vfs_devices);

    let override_entries = if let Some(path) = overrides_path.as_ref() {
        match load_override_file(path) {
            Ok(entries) => {
                info!("Loaded {} override entries", entries.len());
                entries
            }
            Err(err) => {
                error!("Could not load overrides {:?}: {}", path, err);
                std::process::exit(1);
            }
        }
    } else {
        Vec::new()
    };

    info!(
        "Discovering referenced textures (scope: {})...",
        discovery_scope.as_str()
    );
    let (discovered_textures, discovered_zsc_paths) = discover_textures(&vfs, discovery_scope);
    let mut texture_usage: HashMap<VfsPathBuf, TextureUsage> = discovered_textures
        .iter()
        .cloned()
        .map(|path| (path, TextureUsage::default()))
        .collect();

    if !discovered_zsc_paths.is_empty() {
        info!(
            "Scanning {} ZSC files for usage metadata...",
            discovered_zsc_paths.len()
        );
        apply_zsc_usage(&vfs, &discovered_zsc_paths, &mut texture_usage);
    }

    let override_entries_applied = apply_overrides(&override_entries, &mut texture_usage);
    let excluded_textures = apply_builtin_exclusions(&mut texture_usage);
    if !excluded_textures.is_empty() {
        info!(
            "Skipped {} known problematic textures via built-in exclusions",
            excluded_textures.len()
        );
    }
    let mut discovered_sorted: Vec<_> = texture_usage.keys().cloned().collect();
    discovered_sorted.sort_by(|a, b| a.path().cmp(b.path()));

    info!(
        "Found {} unique DDS textures referenced by discovered assets",
        texture_usage.len()
    );

    let specular_count = texture_usage.values().filter(|u| u.specular).count();
    let cutout_count = texture_usage
        .values()
        .filter(|u| !u.specular && u.cutout)
        .count();
    let regular_count = texture_usage.len() - specular_count - cutout_count;
    info!("  {} textures with specular alpha", specular_count);
    info!("  {} textures with alpha cutout", cutout_count);
    info!("  {} regular textures", regular_count);

    if dry_run {
        info!("Dry run mode - listing textures:");
        for path in &discovered_sorted {
            let usage = texture_usage.get(path).copied().unwrap_or_default();
            let mut flags = Vec::new();
            if usage.specular {
                flags.push("SPECULAR");
            }
            if usage.cutout {
                flags.push("CUTOUT");
            }
            if usage.overridden {
                flags.push("OVERRIDE");
            }

            if flags.is_empty() {
                info!("  {}", path.path().display());
            } else {
                info!("  {} [{}]", path.path().display(), flags.join(", "));
            }
        }

        if let Some(report_path) = report_path {
            let report = MipgenReport {
                discovery_scope: discovery_scope.as_str().to_string(),
                filter: filter.as_str().to_string(),
                textures_discovered: texture_usage.len(),
                specular_textures: specular_count,
                cutout_textures: cutout_count,
                regular_textures: regular_count,
                generated_mipmaps: 0,
                skipped_existing: 0,
                skipped_excluded: excluded_textures.len() as u32,
                errors: 0,
                excluded_textures: excluded_textures.clone(),
                missing_textures: Vec::new(),
                unsupported_formats: Vec::new(),
                processing_errors: Vec::new(),
                override_entries_applied,
            };
            if let Err(err) = write_report_json(&report_path, &report) {
                warn!("Could not write report {:?}: {}", report_path, err);
            } else {
                info!("Wrote report {:?}", report_path);
            }
        }
        return;
    }

    remove_excluded_outputs(&output_path, &excluded_textures);

    let mut success_count = 0u32;
    let mut skip_count = 0u32;
    let mut error_count = 0u32;

    let mut missing_textures: Vec<String> = Vec::new();
    let mut unsupported_formats: HashSet<String> = HashSet::new();
    let mut processing_errors: Vec<ProcessingError> = Vec::new();

    for texture_path in &discovered_sorted {
        let usage = texture_usage.get(texture_path).copied().unwrap_or_default();
        let path_str = texture_path.path().to_string_lossy().to_string();

        let data = match vfs.open_file(texture_path.path()) {
            Ok(VfsFile::Buffer(buf)) => buf,
            Ok(VfsFile::View(view)) => view.to_vec(),
            Err(e) => {
                warn!("Could not open {}: {}", path_str, e);
                missing_textures.push(path_str.clone());
                processing_errors.push(ProcessingError {
                    path: path_str.clone(),
                    stage: "open".to_string(),
                    error: e.to_string(),
                });
                error_count += 1;
                continue;
            }
        };

        match process_dds_texture(&data, usage, filter) {
            Ok(Some(new_dds_data)) => {
                let out_file_path = output_path.join(
                    texture_path
                        .path()
                        .to_string_lossy()
                        .replace('\\', "/")
                        .trim_start_matches('/'),
                );

                if let Some(parent) = out_file_path.parent() {
                    if let Err(e) = fs::create_dir_all(parent) {
                        error!("Could not create directory {:?}: {}", parent, e);
                        processing_errors.push(ProcessingError {
                            path: path_str.clone(),
                            stage: "create_dir".to_string(),
                            error: e.to_string(),
                        });
                        error_count += 1;
                        continue;
                    }
                }

                match fs::write(&out_file_path, &new_dds_data) {
                    Ok(()) => {
                        info!(
                            "Generated mipmaps for {}{}",
                            path_str,
                            if usage.specular {
                                " (specular alpha)"
                            } else if usage.cutout {
                                " (cutout alpha)"
                            } else {
                                ""
                            }
                        );
                        success_count += 1;
                    }
                    Err(e) => {
                        error!("Could not write {:?}: {}", out_file_path, e);
                        processing_errors.push(ProcessingError {
                            path: path_str.clone(),
                            stage: "write".to_string(),
                            error: e.to_string(),
                        });
                        error_count += 1;
                    }
                }
            }
            Ok(None) => {
                skip_count += 1;
            }
            Err(e) => {
                warn!("Could not process {}: {}", path_str, e);
                if let Some(unsupported) = extract_unsupported_format(&e) {
                    unsupported_formats.insert(unsupported);
                }
                processing_errors.push(ProcessingError {
                    path: path_str.clone(),
                    stage: "process".to_string(),
                    error: e,
                });
                error_count += 1;
            }
        }
    }

    info!("Done!");
    info!("  Generated mipmaps: {}", success_count);
    info!("  Skipped (already has mipmaps): {}", skip_count);
    info!(
        "  Skipped (built-in exclusions): {}",
        excluded_textures.len()
    );
    info!("  Errors: {}", error_count);

    if let Some(report_path) = report_path {
        missing_textures.sort();
        missing_textures.dedup();

        let mut unsupported_formats: Vec<String> = unsupported_formats.into_iter().collect();
        unsupported_formats.sort();

        let report = MipgenReport {
            discovery_scope: discovery_scope.as_str().to_string(),
            filter: filter.as_str().to_string(),
            textures_discovered: texture_usage.len(),
            specular_textures: specular_count,
            cutout_textures: cutout_count,
            regular_textures: regular_count,
            generated_mipmaps: success_count,
            skipped_existing: skip_count,
            skipped_excluded: excluded_textures.len() as u32,
            errors: error_count,
            excluded_textures,
            missing_textures,
            unsupported_formats,
            processing_errors,
            override_entries_applied,
        };

        if let Err(err) = write_report_json(&report_path, &report) {
            warn!("Could not write report {:?}: {}", report_path, err);
        } else {
            info!("Wrote report {:?}", report_path);
        }
    }
}

fn write_report_json(report_path: &Path, report: &MipgenReport) -> Result<(), String> {
    if let Some(parent) = report_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create report directory {:?}: {}", parent, e))?;
    }

    let json = serde_json::to_vec_pretty(report)
        .map_err(|e| format!("Failed to serialize report JSON: {}", e))?;
    fs::write(report_path, json)
        .map_err(|e| format!("Failed to write report file {:?}: {}", report_path, e))
}

fn remove_excluded_outputs(output_path: &Path, excluded_textures: &[String]) {
    for path in excluded_textures {
        let out_file_path = output_path.join(path.replace('\\', "/").trim_start_matches('/'));
        if !out_file_path.exists() {
            continue;
        }

        match fs::remove_file(&out_file_path) {
            Ok(()) => info!(
                "Removed excluded texture from output directory: {}",
                out_file_path.display()
            ),
            Err(err) => warn!(
                "Could not remove excluded texture {:?}: {}",
                out_file_path, err
            ),
        }
    }
}

fn load_override_file(path: &Path) -> Result<Vec<TextureOverrideEntry>, String> {
    let contents = fs::read_to_string(path)
        .map_err(|e| format!("Failed to read overrides file {:?}: {}", path, e))?;
    let parsed: TextureOverrideFile =
        toml::from_str(&contents).map_err(|e| format!("Failed to parse TOML: {}", e))?;
    Ok(parsed.texture)
}

fn apply_overrides(
    override_entries: &[TextureOverrideEntry],
    texture_usage: &mut HashMap<VfsPathBuf, TextureUsage>,
) -> usize {
    let mut override_hits = 0usize;

    for entry in override_entries {
        let key = VfsPathBuf::new(&entry.path);
        let existed = texture_usage.contains_key(&key);

        let usage = texture_usage.entry(key.clone()).or_default();
        match entry.usage {
            TextureOverrideUsage::Specular => {
                usage.specular = true;
                usage.cutout = false;
                usage.regular_alpha = false;
                usage.alpha_cutoff = None;
            }
            TextureOverrideUsage::Cutout => {
                usage.specular = false;
                usage.cutout = true;
                usage.regular_alpha = false;
                let cutoff = entry
                    .alpha_cutoff
                    .unwrap_or(DEFAULT_ALPHA_CUTOFF)
                    .clamp(0.0, 1.0);
                usage.alpha_cutoff = Some(cutoff);
            }
            TextureOverrideUsage::Regular => {
                usage.specular = false;
                usage.cutout = false;
                usage.regular_alpha = true;
                usage.alpha_cutoff = None;
            }
        }
        usage.overridden = true;

        if existed {
            override_hits += 1;
        } else {
            warn!(
                "Override path {} was not found in discovery set; adding explicit processing target",
                key.path().display()
            );
        }
    }

    override_hits
}

fn apply_builtin_exclusions(texture_usage: &mut HashMap<VfsPathBuf, TextureUsage>) -> Vec<String> {
    let mut excluded = Vec::new();

    for path in BUILTIN_EXCLUDED_TEXTURES {
        let key = VfsPathBuf::new(path);
        if texture_usage.remove(&key).is_some() {
            excluded.push((*path).to_string());
            warn!(
                "Skipping known problematic texture via built-in exclusion: {}",
                path
            );
        }
    }

    excluded.sort();
    excluded
}

fn apply_zsc_usage(
    vfs: &VirtualFilesystem,
    zsc_paths: &HashSet<VfsPathBuf>,
    texture_usage: &mut HashMap<VfsPathBuf, TextureUsage>,
) {
    for zsc_path in zsc_paths {
        let Ok(zsc) = vfs.read_file::<ZscFile, _>(zsc_path.path()) else {
            continue;
        };

        for material in &zsc.materials {
            if !is_dds_path(material.path.path()) {
                continue;
            }

            let usage = texture_usage.entry(material.path.clone()).or_default();

            if material.specular_enabled {
                usage.specular = true;
                usage.cutout = false;
                usage.regular_alpha = false;
                usage.alpha_cutoff = None;
                continue;
            }

            // Keep specular classification dominant if this texture is shared
            // by multiple materials with mixed flags.
            if usage.specular {
                continue;
            }

            if let Some(alpha_test) = material.alpha_test {
                usage.cutout = true;
                usage.regular_alpha = false;
                let alpha_test = alpha_test.clamp(0.0, 1.0);
                usage.alpha_cutoff = Some(match usage.alpha_cutoff {
                    Some(existing) => existing.max(alpha_test),
                    None => alpha_test,
                });
            } else if material.alpha_enabled {
                usage.regular_alpha = true;
            }
        }
    }
}

fn is_dds_path(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.eq_ignore_ascii_case("dds"))
        .unwrap_or(false)
}

fn process_dds_texture(
    dds_data: &[u8],
    usage: TextureUsage,
    filter: FilterKind,
) -> Result<Option<Vec<u8>>, String> {
    let dds =
        Dds::read(&mut Cursor::new(dds_data)).map_err(|e| format!("Failed to parse DDS: {}", e))?;

    let width = dds.get_width();
    let height = dds.get_height();

    if width == 0 || height == 0 {
        return Err("Invalid DDS dimensions (0x0)".to_string());
    }

    let mip_count = dds.get_num_mipmap_levels();
    let expected_mips = calculate_mip_count(width, height);

    if mip_count >= expected_mips {
        return Ok(None);
    }

    let tex_format = detect_format(&dds)?;
    let has_alpha = tex_format.has_alpha();
    let alpha_mode = select_alpha_handling(usage, has_alpha);

    let source_level_zero = dds
        .get_data(0)
        .map_err(|e| format!("Failed to get base mip data: {}", e))?;
    let base_level_size = encoded_level_size(width as usize, height as usize, tex_format)?;
    if source_level_zero.len() < base_level_size {
        return Err(format!(
            "Source level 0 too small, expected at least {}, got {}",
            base_level_size,
            source_level_zero.len()
        ));
    }
    // Some DDS files include additional mip bytes in level-0 view; keep only real mip0.
    let original_base_data = &source_level_zero[..base_level_size];

    let base_rgba = decode_to_rgba(
        original_base_data,
        width as usize,
        height as usize,
        tex_format,
    )?;

    let mut mip_rgba_levels: Vec<Vec<u8>> = Vec::new();
    mip_rgba_levels.push(base_rgba);

    let mut mip_w = width as usize;
    let mut mip_h = height as usize;

    for _ in 1..expected_mips {
        let new_w = (mip_w / 2).max(1);
        let new_h = (mip_h / 2).max(1);

        let prev_rgba = mip_rgba_levels.last().expect("mip levels never empty");
        let downsampled =
            downsample_rgba_with_mode(prev_rgba, mip_w, mip_h, new_w, new_h, filter, alpha_mode);

        mip_rgba_levels.push(downsampled);
        mip_w = new_w;
        mip_h = new_h;
    }

    let mut all_mip_data: Vec<u8> = Vec::new();
    all_mip_data.extend_from_slice(original_base_data);

    for level in 1..expected_mips {
        let w = (width >> level).max(1) as usize;
        let h = (height >> level).max(1) as usize;
        let rgba = &mip_rgba_levels[level as usize];
        let encoded = encode_from_rgba(rgba, w, h, tex_format)?;
        all_mip_data.extend_from_slice(&encoded);
    }

    let new_dds = build_dds_with_mipmaps(width, height, expected_mips, tex_format, &all_mip_data)?;

    let mut output = Vec::new();
    new_dds
        .write(&mut output)
        .map_err(|e| format!("Failed to write DDS: {}", e))?;

    Ok(Some(output))
}

fn select_alpha_handling(usage: TextureUsage, has_alpha: bool) -> AlphaHandling {
    if !has_alpha {
        return AlphaHandling::Regular;
    }

    if usage.specular {
        AlphaHandling::Specular
    } else if usage.cutout {
        AlphaHandling::Cutout(
            usage
                .alpha_cutoff
                .unwrap_or(DEFAULT_ALPHA_CUTOFF)
                .clamp(0.0, 1.0),
        )
    } else if usage.regular_alpha {
        AlphaHandling::Regular
    } else {
        // Unknown alpha usage defaults to separate-channel filtering to avoid
        // dark/black artifacts from accidental premultiply on non-transparent data.
        AlphaHandling::Specular
    }
}

fn calculate_mip_count(width: u32, height: u32) -> u32 {
    let max_dim = width.max(height);
    (max_dim as f32).log2().floor() as u32 + 1
}

fn detect_format(dds: &Dds) -> Result<TexFormat, String> {
    if let Some(d3d_format) = dds.get_d3d_format() {
        return match d3d_format {
            D3DFormat::DXT1 => Ok(TexFormat::Compressed(Format::Bc1)),
            D3DFormat::DXT3 => Ok(TexFormat::Compressed(Format::Bc2)),
            D3DFormat::DXT5 => Ok(TexFormat::Compressed(Format::Bc3)),
            D3DFormat::A8R8G8B8 => Ok(TexFormat::A8R8G8B8),
            D3DFormat::X8R8G8B8 => Ok(TexFormat::X8R8G8B8),
            D3DFormat::R8G8B8 => Ok(TexFormat::R8G8B8),
            D3DFormat::A4R4G4B4 => Ok(TexFormat::A4R4G4B4),
            D3DFormat::R5G6B5 => Ok(TexFormat::R5G6B5),
            D3DFormat::A1R5G5B5 => Ok(TexFormat::A1R5G5B5),
            other => Err(format!("Unsupported D3D format: {:?}", other)),
        };
    }

    if let Some(dxgi_format) = dds.get_dxgi_format() {
        return match dxgi_format {
            DxgiFormat::BC1_UNorm | DxgiFormat::BC1_UNorm_sRGB => {
                Ok(TexFormat::Compressed(Format::Bc1))
            }
            DxgiFormat::BC2_UNorm | DxgiFormat::BC2_UNorm_sRGB => {
                Ok(TexFormat::Compressed(Format::Bc2))
            }
            DxgiFormat::BC3_UNorm | DxgiFormat::BC3_UNorm_sRGB => {
                Ok(TexFormat::Compressed(Format::Bc3))
            }
            DxgiFormat::B8G8R8A8_UNorm | DxgiFormat::B8G8R8A8_UNorm_sRGB => Ok(TexFormat::A8R8G8B8),
            DxgiFormat::B8G8R8X8_UNorm | DxgiFormat::B8G8R8X8_UNorm_sRGB => Ok(TexFormat::X8R8G8B8),
            DxgiFormat::B5G6R5_UNorm => Ok(TexFormat::R5G6B5),
            DxgiFormat::B5G5R5A1_UNorm => Ok(TexFormat::A1R5G5B5),
            DxgiFormat::B4G4R4A4_UNorm => Ok(TexFormat::A4R4G4B4),
            other => Err(format!("Unsupported DXGI format: {:?}", other)),
        };
    }

    Err("Could not determine DDS format".to_string())
}

fn decode_to_rgba(
    data: &[u8],
    width: usize,
    height: usize,
    format: TexFormat,
) -> Result<Vec<u8>, String> {
    match format {
        TexFormat::Compressed(bc_format) => {
            let mut output = vec![0u8; width * height * 4];
            bc_format.decompress(data, width, height, &mut output);
            Ok(output)
        }
        TexFormat::A8R8G8B8 => {
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
        TexFormat::X8R8G8B8 => {
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
        TexFormat::R8G8B8 => {
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
        TexFormat::A4R4G4B4 => {
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
        TexFormat::R5G6B5 => {
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
        TexFormat::A1R5G5B5 => {
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

fn encode_from_rgba(
    rgba: &[u8],
    width: usize,
    height: usize,
    format: TexFormat,
) -> Result<Vec<u8>, String> {
    match format {
        TexFormat::Compressed(bc_format) => compress_bc(rgba, width, height, bc_format),
        TexFormat::A8R8G8B8 => {
            let mut output = vec![0u8; width * height * 4];
            for i in 0..(width * height) {
                let si = i * 4;
                output[si] = rgba[si + 2];
                output[si + 1] = rgba[si + 1];
                output[si + 2] = rgba[si];
                output[si + 3] = rgba[si + 3];
            }
            Ok(output)
        }
        TexFormat::X8R8G8B8 => {
            let mut output = vec![0u8; width * height * 4];
            for i in 0..(width * height) {
                let si = i * 4;
                output[si] = rgba[si + 2];
                output[si + 1] = rgba[si + 1];
                output[si + 2] = rgba[si];
                output[si + 3] = 0xFF;
            }
            Ok(output)
        }
        TexFormat::R8G8B8 => {
            let mut output = vec![0u8; width * height * 3];
            for i in 0..(width * height) {
                let si = i * 4;
                let di = i * 3;
                output[di] = rgba[si + 2];
                output[di + 1] = rgba[si + 1];
                output[di + 2] = rgba[si];
            }
            Ok(output)
        }
        TexFormat::A4R4G4B4 => {
            let mut output = vec![0u8; width * height * 2];
            for i in 0..(width * height) {
                let si = i * 4;
                let r = (rgba[si] as u16) >> 4;
                let g = (rgba[si + 1] as u16) >> 4;
                let b = (rgba[si + 2] as u16) >> 4;
                let a = (rgba[si + 3] as u16) >> 4;
                let pixel = b | (g << 4) | (r << 8) | (a << 12);
                let di = i * 2;
                output[di] = pixel as u8;
                output[di + 1] = (pixel >> 8) as u8;
            }
            Ok(output)
        }
        TexFormat::R5G6B5 => {
            let mut output = vec![0u8; width * height * 2];
            for i in 0..(width * height) {
                let si = i * 4;
                let r = (rgba[si] as u16) >> 3;
                let g = (rgba[si + 1] as u16) >> 2;
                let b = (rgba[si + 2] as u16) >> 3;
                let pixel = b | (g << 5) | (r << 11);
                let di = i * 2;
                output[di] = pixel as u8;
                output[di + 1] = (pixel >> 8) as u8;
            }
            Ok(output)
        }
        TexFormat::A1R5G5B5 => {
            let mut output = vec![0u8; width * height * 2];
            for i in 0..(width * height) {
                let si = i * 4;
                let r = (rgba[si] as u16) >> 3;
                let g = (rgba[si + 1] as u16) >> 3;
                let b = (rgba[si + 2] as u16) >> 3;
                let a: u16 = if rgba[si + 3] >= 128 { 1 } else { 0 };
                let pixel = b | (g << 5) | (r << 10) | (a << 15);
                let di = i * 2;
                output[di] = pixel as u8;
                output[di + 1] = (pixel >> 8) as u8;
            }
            Ok(output)
        }
    }
}

fn compress_bc(
    rgba: &[u8],
    width: usize,
    height: usize,
    format: Format,
) -> Result<Vec<u8>, String> {
    let block_count_x = (width + 3) / 4;
    let block_count_y = (height + 3) / 4;
    let bytes_per_block = match format {
        Format::Bc1 => 8,
        Format::Bc2 | Format::Bc3 => 16,
        _ => return Err(format!("Unsupported format for compression: {:?}", format)),
    };
    let output_size = block_count_x * block_count_y * bytes_per_block;
    let mut output = vec![0u8; output_size];

    let params = texpresso::Params {
        algorithm: texpresso::Algorithm::IterativeClusterFit,
        weights: texpresso::COLOUR_WEIGHTS_PERCEPTUAL,
        weigh_colour_by_alpha: format != Format::Bc1,
    };

    format.compress(rgba, width, height, params, &mut output);
    Ok(output)
}

fn encoded_level_size(width: usize, height: usize, format: TexFormat) -> Result<usize, String> {
    if width == 0 || height == 0 {
        return Err("Invalid mip dimensions 0x0".to_string());
    }

    let size = match format {
        TexFormat::Compressed(bc_format) => {
            let block_count_x = (width + 3) / 4;
            let block_count_y = (height + 3) / 4;
            let bytes_per_block = match bc_format {
                Format::Bc1 => 8usize,
                Format::Bc2 | Format::Bc3 => 16usize,
                _ => return Err(format!("Unsupported BC format for sizing: {:?}", bc_format)),
            };
            block_count_x
                .checked_mul(block_count_y)
                .and_then(|n| n.checked_mul(bytes_per_block))
        }
        TexFormat::A8R8G8B8 | TexFormat::X8R8G8B8 => {
            width.checked_mul(height).and_then(|n| n.checked_mul(4))
        }
        TexFormat::R8G8B8 => width.checked_mul(height).and_then(|n| n.checked_mul(3)),
        TexFormat::A4R4G4B4 | TexFormat::R5G6B5 | TexFormat::A1R5G5B5 => {
            width.checked_mul(height).and_then(|n| n.checked_mul(2))
        }
    };

    size.ok_or_else(|| "Mip size overflow".to_string())
}

fn downsample_rgba_with_mode(
    rgba: &[u8],
    src_w: usize,
    src_h: usize,
    dst_w: usize,
    dst_h: usize,
    filter: FilterKind,
    alpha_mode: AlphaHandling,
) -> Vec<u8> {
    let pixel_count = src_w * src_h;

    let mut r = vec![0.0f32; pixel_count];
    let mut g = vec![0.0f32; pixel_count];
    let mut b = vec![0.0f32; pixel_count];
    let mut a = vec![0.0f32; pixel_count];

    for i in 0..pixel_count {
        let si = i * 4;
        r[i] = rgba[si] as f32 / 255.0;
        g[i] = rgba[si + 1] as f32 / 255.0;
        b[i] = rgba[si + 2] as f32 / 255.0;
        a[i] = rgba[si + 3] as f32 / 255.0;
    }
    let (r2, g2, b2, a2) = match alpha_mode {
        AlphaHandling::Specular => (
            resample_plane(&r, src_w, src_h, dst_w, dst_h, filter),
            resample_plane(&g, src_w, src_h, dst_w, dst_h, filter),
            resample_plane(&b, src_w, src_h, dst_w, dst_h, filter),
            resample_plane(&a, src_w, src_h, dst_w, dst_h, filter),
        ),
        AlphaHandling::Regular => {
            let mut rp = vec![0.0f32; pixel_count];
            let mut gp = vec![0.0f32; pixel_count];
            let mut bp = vec![0.0f32; pixel_count];

            for i in 0..pixel_count {
                rp[i] = r[i] * a[i];
                gp[i] = g[i] * a[i];
                bp[i] = b[i] * a[i];
            }

            let rp2 = resample_plane(&rp, src_w, src_h, dst_w, dst_h, filter);
            let gp2 = resample_plane(&gp, src_w, src_h, dst_w, dst_h, filter);
            let bp2 = resample_plane(&bp, src_w, src_h, dst_w, dst_h, filter);
            let a2 = resample_plane(&a, src_w, src_h, dst_w, dst_h, filter);

            unpremultiply_rgb(rp2, gp2, bp2, a2)
        }
        AlphaHandling::Cutout(cutoff) => {
            let mut rp = vec![0.0f32; pixel_count];
            let mut gp = vec![0.0f32; pixel_count];
            let mut bp = vec![0.0f32; pixel_count];

            for i in 0..pixel_count {
                rp[i] = r[i] * a[i];
                gp[i] = g[i] * a[i];
                bp[i] = b[i] * a[i];
            }

            let rp2 = resample_plane(&rp, src_w, src_h, dst_w, dst_h, filter);
            let gp2 = resample_plane(&gp, src_w, src_h, dst_w, dst_h, filter);
            let bp2 = resample_plane(&bp, src_w, src_h, dst_w, dst_h, filter);
            let mut a2 = resample_plane(&a, src_w, src_h, dst_w, dst_h, filter);

            preserve_cutout_coverage(&a, &mut a2, cutoff);
            unpremultiply_rgb(rp2, gp2, bp2, a2)
        }
    };

    let mut output = vec![0u8; dst_w * dst_h * 4];
    for i in 0..(dst_w * dst_h) {
        let di = i * 4;
        output[di] = to_u8(r2[i]);
        output[di + 1] = to_u8(g2[i]);
        output[di + 2] = to_u8(b2[i]);
        output[di + 3] = to_u8(a2[i]);
    }

    output
}

fn unpremultiply_rgb(
    rp2: Vec<f32>,
    gp2: Vec<f32>,
    bp2: Vec<f32>,
    a2: Vec<f32>,
) -> (Vec<f32>, Vec<f32>, Vec<f32>, Vec<f32>) {
    let mut r2 = vec![0.0f32; a2.len()];
    let mut g2 = vec![0.0f32; a2.len()];
    let mut b2 = vec![0.0f32; a2.len()];

    for i in 0..a2.len() {
        let alpha = a2[i].clamp(0.0, 1.0);
        if alpha > 1e-6 {
            r2[i] = (rp2[i] / alpha).clamp(0.0, 1.0);
            g2[i] = (gp2[i] / alpha).clamp(0.0, 1.0);
            b2[i] = (bp2[i] / alpha).clamp(0.0, 1.0);
        }
    }

    (r2, g2, b2, a2)
}

fn preserve_cutout_coverage(src_alpha: &[f32], dst_alpha: &mut [f32], cutoff: f32) {
    if src_alpha.is_empty() || dst_alpha.is_empty() {
        return;
    }

    let cutoff = cutoff.clamp(0.0, 1.0);
    let source_coverage = alpha_coverage(src_alpha, cutoff);

    if source_coverage <= 0.0 || source_coverage >= 1.0 {
        return;
    }

    let current_coverage = alpha_coverage(dst_alpha, cutoff);
    if (current_coverage - source_coverage).abs() <= 0.001 {
        return;
    }

    let mut low = 0.0f32;
    let mut high = 8.0f32;

    for _ in 0..24 {
        let mid = (low + high) * 0.5;
        let cov = alpha_coverage_scaled(dst_alpha, cutoff, mid);
        if cov < source_coverage {
            low = mid;
        } else {
            high = mid;
        }
    }

    let scale = high;
    for alpha in dst_alpha {
        *alpha = (*alpha * scale).clamp(0.0, 1.0);
    }
}

fn alpha_coverage(alpha: &[f32], cutoff: f32) -> f32 {
    let hit = alpha.iter().filter(|a| **a >= cutoff).count();
    hit as f32 / alpha.len() as f32
}

fn alpha_coverage_scaled(alpha: &[f32], cutoff: f32, scale: f32) -> f32 {
    let hit = alpha
        .iter()
        .filter(|a| (**a * scale).clamp(0.0, 1.0) >= cutoff)
        .count();
    hit as f32 / alpha.len() as f32
}

#[derive(Clone)]
struct Contributor {
    index: usize,
    weight: f32,
}

fn resample_plane(
    source: &[f32],
    src_w: usize,
    src_h: usize,
    dst_w: usize,
    dst_h: usize,
    filter: FilterKind,
) -> Vec<f32> {
    if src_w == dst_w && src_h == dst_h {
        return source.to_vec();
    }

    let contrib_x = build_contributors(src_w, dst_w, filter);
    let contrib_y = build_contributors(src_h, dst_h, filter);

    let mut temp = vec![0.0f32; dst_w * src_h];
    for y in 0..src_h {
        let src_row = &source[y * src_w..(y + 1) * src_w];
        let dst_row = &mut temp[y * dst_w..(y + 1) * dst_w];

        for x in 0..dst_w {
            let mut sum = 0.0f32;
            for c in &contrib_x[x] {
                sum += src_row[c.index] * c.weight;
            }
            dst_row[x] = sum;
        }
    }

    let mut out = vec![0.0f32; dst_w * dst_h];
    for y in 0..dst_h {
        let row_weights = &contrib_y[y];
        for x in 0..dst_w {
            let mut sum = 0.0f32;
            for c in row_weights {
                sum += temp[c.index * dst_w + x] * c.weight;
            }
            out[y * dst_w + x] = sum;
        }
    }

    out
}

fn build_contributors(
    src_size: usize,
    dst_size: usize,
    filter: FilterKind,
) -> Vec<Vec<Contributor>> {
    let scale = src_size as f32 / dst_size as f32;
    let filter_scale = scale.max(1.0);
    let support = kernel_radius(filter) * filter_scale;

    let mut table = Vec::with_capacity(dst_size);
    for dst_i in 0..dst_size {
        let src_center = (dst_i as f32 + 0.5) * scale - 0.5;
        let left = (src_center - support).floor() as isize;
        let right = (src_center + support).ceil() as isize;

        let mut taps = Vec::new();
        let mut weight_sum = 0.0f32;

        for src_i in left..=right {
            let distance = src_center - src_i as f32;
            let weight = kernel_value(filter, distance / filter_scale);
            if weight.abs() < 1e-8 {
                continue;
            }

            let clamped = clamp_index(src_i, src_size);
            taps.push(Contributor {
                index: clamped,
                weight,
            });
            weight_sum += weight;
        }

        if taps.is_empty() || weight_sum.abs() < 1e-8 {
            table.push(vec![Contributor {
                index: clamp_index(src_center.round() as isize, src_size),
                weight: 1.0,
            }]);
            continue;
        }

        for tap in &mut taps {
            tap.weight /= weight_sum;
        }

        table.push(taps);
    }

    table
}
fn clamp_index(index: isize, size: usize) -> usize {
    if index <= 0 {
        0
    } else if index >= (size as isize - 1) {
        size - 1
    } else {
        index as usize
    }
}

fn kernel_radius(filter: FilterKind) -> f32 {
    match filter {
        FilterKind::Box => 0.5,
        FilterKind::Lanczos3 => 3.0,
        FilterKind::Kaiser => KAISER_RADIUS,
    }
}

fn kernel_value(filter: FilterKind, x: f32) -> f32 {
    match filter {
        FilterKind::Box => {
            if x.abs() <= 0.5 {
                1.0
            } else {
                0.0
            }
        }
        FilterKind::Lanczos3 => lanczos_kernel(x, 3.0),
        FilterKind::Kaiser => kaiser_windowed_sinc(x, KAISER_RADIUS, KAISER_BETA),
    }
}

fn lanczos_kernel(x: f32, a: f32) -> f32 {
    let ax = x.abs();
    if ax >= a {
        0.0
    } else {
        sinc(x) * sinc(x / a)
    }
}

fn kaiser_windowed_sinc(x: f32, radius: f32, beta: f32) -> f32 {
    let ax = x.abs();
    if ax >= radius {
        return 0.0;
    }

    let ratio = (ax / radius).min(1.0);
    let window_arg = (1.0 - ratio * ratio).max(0.0).sqrt();
    let window = bessel_i0(beta * window_arg) / bessel_i0(beta);
    sinc(x) * window
}

fn sinc(x: f32) -> f32 {
    if x.abs() < 1e-6 {
        1.0
    } else {
        let px = PI * x;
        px.sin() / px
    }
}

fn bessel_i0(x: f32) -> f32 {
    let ax = x.abs();
    if ax < 3.75 {
        let y = (x / 3.75) * (x / 3.75);
        1.0 + y
            * (3.515_622_9
                + y * (3.089_942_4
                    + y * (1.206_749_2 + y * (0.265_973_2 + y * (0.036_076_8 + y * 0.004_581_3)))))
    } else {
        let y = 3.75 / ax;
        (ax.exp() / ax.sqrt())
            * (0.398_942_3
                + y * (0.013_285_92
                    + y * (0.002_253_19
                        + y * (-0.001_575_65
                            + y * (0.009_162_81
                                + y * (-0.020_577_06
                                    + y * (0.026_355_37
                                        + y * (-0.016_476_33 + y * 0.003_923_77))))))))
    }
}

fn to_u8(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0 + 0.5).floor() as u8
}

fn build_dds_with_mipmaps(
    width: u32,
    height: u32,
    mip_count: u32,
    format: TexFormat,
    data: &[u8],
) -> Result<Dds, String> {
    let d3d_format = match format {
        TexFormat::Compressed(Format::Bc1) => D3DFormat::DXT1,
        TexFormat::Compressed(Format::Bc2) => D3DFormat::DXT3,
        TexFormat::Compressed(Format::Bc3) => D3DFormat::DXT5,
        TexFormat::Compressed(_) => return Err("Unsupported BC format".to_string()),
        TexFormat::A8R8G8B8 => D3DFormat::A8R8G8B8,
        TexFormat::X8R8G8B8 => D3DFormat::X8R8G8B8,
        TexFormat::R8G8B8 => D3DFormat::R8G8B8,
        TexFormat::A4R4G4B4 => D3DFormat::A4R4G4B4,
        TexFormat::R5G6B5 => D3DFormat::R5G6B5,
        TexFormat::A1R5G5B5 => D3DFormat::A1R5G5B5,
    };

    let mut dds = Dds::new_d3d(ddsfile::NewD3dParams {
        height,
        width,
        depth: None,
        format: d3d_format,
        mipmap_levels: Some(mip_count),
        caps2: None,
    })
    .map_err(|e| format!("Failed to create DDS: {}", e))?;

    let dds_data = dds
        .get_mut_data(0)
        .map_err(|e| format!("Failed to get mutable DDS data: {}", e))?;

    if data.len() != dds_data.len() {
        return Err(format!(
            "Mip data length mismatch, expected {}, got {}",
            dds_data.len(),
            data.len()
        ));
    }

    dds_data.copy_from_slice(data);

    Ok(dds)
}

fn extract_unsupported_format(error: &str) -> Option<String> {
    if error.starts_with("Unsupported ") {
        Some(error.to_string())
    } else {
        None
    }
}
