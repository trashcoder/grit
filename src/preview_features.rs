use crate::GritApp;
use eframe::egui;
use git2::{ObjectType, Oid, Repository, Tree};
use image::{ImageReader, Limits};
use std::{
    collections::HashMap,
    hash::{Hash, Hasher},
    io::{Cursor, Read},
    path::Path,
};

const MAX_IMAGE_BYTES: usize = 16 * 1024 * 1024;
const MAX_TEXT_BYTES: usize = 4 * 1024 * 1024;
const MAX_IMAGE_PIXELS: u64 = 16_000_000;
const MAX_IMAGE_DECODE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_TEXTURE_CACHE: usize = 32;
const MAX_TEXTURE_CACHE_BYTES: usize = 128 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TextEncoding {
    #[default]
    Utf8,
    Latin1,
    Windows1252,
    Utf16Le,
    Utf16Be,
}

#[derive(Default)]
pub struct PreviewTools {
    pub encoding: TextEncoding,
    pair_cache: Option<(String, Result<PreviewPair, String>)>,
    texture_cache: TextureCache,
    decoded_cache: DecodedTextCache,
}

#[derive(Default)]
struct TextureCache {
    textures: HashMap<String, Result<egui::TextureHandle, String>>,
    texture_order: Vec<String>,
    texture_bytes: HashMap<String, usize>,
    total_texture_bytes: usize,
}

#[derive(Default)]
struct DecodedTextCache {
    texts: HashMap<String, Result<DecodedText, String>>,
    order: Vec<String>,
    sizes: HashMap<String, usize>,
    total_bytes: usize,
}

struct DecodedText {
    text: String,
    line_starts: Vec<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreviewBlob {
    pub key: String,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PreviewPair {
    pub old: Option<PreviewBlob>,
    pub new: Option<PreviewBlob>,
}

#[derive(Clone, Copy)]
struct PreviewBlobRef<'a> {
    key: &'a str,
    bytes: &'a [u8],
}

impl PreviewTools {
    /// Clear source bytes after the repository has been refreshed or mutated.
    pub fn invalidate(&mut self) {
        self.pair_cache = None;
    }

    pub fn encoding_selector(&mut self, ui: &mut egui::Ui) {
        egui::ComboBox::from_id_salt("preview_text_encoding")
            .selected_text(self.encoding.label())
            .show_ui(ui, |ui| {
                for encoding in [
                    TextEncoding::Utf8,
                    TextEncoding::Latin1,
                    TextEncoding::Windows1252,
                    TextEncoding::Utf16Le,
                    TextEncoding::Utf16Be,
                ] {
                    ui.selectable_value(&mut self.encoding, encoding, encoding.label());
                }
            });
    }
}

impl DecodedTextCache {
    fn ensure(&mut self, key: String, bytes: &[u8], encoding: TextEncoding) {
        if self.texts.contains_key(&key) {
            return;
        }
        let decoded = decode_text(bytes, encoding).map(|text| {
            let mut line_starts = vec![0];
            for (index, byte) in text.bytes().enumerate() {
                if byte == b'\n' && index + 1 < text.len() {
                    line_starts.push((index + 1) as u32);
                }
            }
            DecodedText { text, line_starts }
        });
        let size = decoded.as_ref().map_or(0, |decoded| {
            decoded.text.len() + decoded.line_starts.len() * std::mem::size_of::<u32>()
        });
        self.order.push(key.clone());
        self.sizes.insert(key.clone(), size);
        self.total_bytes = self.total_bytes.saturating_add(size);
        self.texts.insert(key, decoded);
        while self.order.len() > 8 || self.total_bytes > 48 * 1024 * 1024 {
            if let Some(expired) = self.order.first().cloned() {
                self.order.remove(0);
                self.texts.remove(&expired);
                if let Some(size) = self.sizes.remove(&expired) {
                    self.total_bytes = self.total_bytes.saturating_sub(size);
                }
            } else {
                break;
            }
        }
    }
}

impl TextureCache {
    fn texture(
        &mut self,
        ctx: &egui::Context,
        key: String,
        bytes: &[u8],
    ) -> Result<egui::TextureHandle, String> {
        if let Some(cached) = self.textures.get(&key) {
            return cached.clone();
        }
        let decoded = decode_limited_image(bytes)?;
        let size = [decoded.width() as usize, decoded.height() as usize];
        let image = egui::ColorImage::from_rgba_unmultiplied(size, decoded.as_raw());
        let texture = ctx.load_texture(
            format!("image-preview:{key}"),
            image,
            egui::TextureOptions::LINEAR,
        );
        let bytes_used = size[0].saturating_mul(size[1]).saturating_mul(4);
        self.texture_order.push(key.clone());
        self.texture_bytes.insert(key.clone(), bytes_used);
        self.total_texture_bytes = self.total_texture_bytes.saturating_add(bytes_used);
        self.textures.insert(key.clone(), Ok(texture.clone()));
        self.evict_textures();
        Ok(texture)
    }

    fn cached_error(&mut self, key: String, error: String) {
        if self.textures.contains_key(&key) {
            return;
        }
        self.texture_order.push(key.clone());
        self.textures.insert(key, Err(error));
        self.evict_textures();
    }

    fn evict_textures(&mut self) {
        while self.texture_order.len() > MAX_TEXTURE_CACHE
            || self.total_texture_bytes > MAX_TEXTURE_CACHE_BYTES
        {
            if let Some(expired) = self.texture_order.first().cloned() {
                self.texture_order.remove(0);
                self.textures.remove(&expired);
                if let Some(size) = self.texture_bytes.remove(&expired) {
                    self.total_texture_bytes = self.total_texture_bytes.saturating_sub(size);
                }
            } else {
                break;
            }
        }
    }
}

impl TextEncoding {
    pub fn label(self) -> &'static str {
        match self {
            Self::Utf8 => "UTF-8",
            Self::Latin1 => "Latin-1",
            Self::Windows1252 => "Windows-1252",
            Self::Utf16Le => "UTF-16 LE",
            Self::Utf16Be => "UTF-16 BE",
        }
    }
}

/// Decode text without replacing malformed input. A recognized BOM selects its encoding.
pub fn decode_text(bytes: &[u8], selected: TextEncoding) -> Result<String, String> {
    if let Some(rest) = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]) {
        return std::str::from_utf8(rest)
            .map(str::to_owned)
            .map_err(|e| format!("Invalid UTF-8 at byte {}", e.valid_up_to()));
    }
    if let Some(rest) = bytes.strip_prefix(&[0xff, 0xfe]) {
        return decode_utf16(rest, true);
    }
    if let Some(rest) = bytes.strip_prefix(&[0xfe, 0xff]) {
        return decode_utf16(rest, false);
    }
    match selected {
        TextEncoding::Utf8 => std::str::from_utf8(bytes)
            .map(str::to_owned)
            .map_err(|e| format!("Invalid UTF-8 at byte {}", e.valid_up_to())),
        TextEncoding::Latin1 => Ok(bytes.iter().map(|&b| char::from(b)).collect()),
        TextEncoding::Windows1252 => decode_windows_1252(bytes),
        TextEncoding::Utf16Le => decode_utf16(bytes, true),
        TextEncoding::Utf16Be => decode_utf16(bytes, false),
    }
}

#[cfg(test)]
fn encode_text(text: &str, encoding: TextEncoding, with_bom: bool) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    match encoding {
        TextEncoding::Utf8 => {
            if with_bom {
                out.extend_from_slice(&[0xef, 0xbb, 0xbf]);
            }
            out.extend_from_slice(text.as_bytes());
        }
        TextEncoding::Latin1 => {
            for ch in text.chars() {
                let value = u32::from(ch);
                if value > 0xff {
                    return Err(format!(
                        "Character U+{value:04X} is not representable in Latin-1"
                    ));
                }
                out.push(value as u8);
            }
        }
        TextEncoding::Windows1252 => {
            for ch in text.chars() {
                let value = u32::from(ch);
                if value <= 0x7f || (0xa0..=0xff).contains(&value) {
                    out.push(value as u8);
                } else if let Some(byte) = CP1252_EXTENDED
                    .iter()
                    .position(|&code| code == value as u16)
                {
                    out.push(0x80 + byte as u8);
                } else {
                    return Err(format!(
                        "Character U+{value:04X} is not representable in Windows-1252"
                    ));
                }
            }
        }
        TextEncoding::Utf16Le | TextEncoding::Utf16Be => {
            if with_bom {
                out.extend_from_slice(if encoding == TextEncoding::Utf16Le {
                    &[0xff, 0xfe]
                } else {
                    &[0xfe, 0xff]
                });
            }
            for unit in text.encode_utf16() {
                let unit_bytes = if encoding == TextEncoding::Utf16Le {
                    unit.to_le_bytes()
                } else {
                    unit.to_be_bytes()
                };
                out.extend_from_slice(&unit_bytes);
            }
        }
    }
    Ok(out)
}

fn decode_utf16(bytes: &[u8], little_endian: bool) -> Result<String, String> {
    let (pairs, remainder) = bytes.as_chunks::<2>();
    if !remainder.is_empty() {
        return Err("UTF-16 input has an incomplete code unit".into());
    }
    let units = pairs.iter().map(|&pair| {
        if little_endian {
            u16::from_le_bytes(pair)
        } else {
            u16::from_be_bytes(pair)
        }
    });
    char::decode_utf16(units)
        .collect::<Result<String, _>>()
        .map_err(|e| format!("Invalid UTF-16 surrogate U+{:04X}", e.unpaired_surrogate()))
}

const CP1252_EXTENDED: [u16; 32] = [
    0x20ac, 0x0081, 0x201a, 0x0192, 0x201e, 0x2026, 0x2020, 0x2021, 0x02c6, 0x2030, 0x0160, 0x2039,
    0x0152, 0x008d, 0x017d, 0x008f, 0x0090, 0x2018, 0x2019, 0x201c, 0x201d, 0x2022, 0x2013, 0x2014,
    0x02dc, 0x2122, 0x0161, 0x203a, 0x0153, 0x009d, 0x017e, 0x0178,
];

fn decode_windows_1252(bytes: &[u8]) -> Result<String, String> {
    let mut out = String::with_capacity(bytes.len());
    for &byte in bytes {
        let code = if (0x80..=0x9f).contains(&byte) {
            CP1252_EXTENDED[(byte - 0x80) as usize]
        } else {
            u16::from(byte)
        };
        if matches!(code, 0x0081 | 0x008d | 0x008f | 0x0090 | 0x009d) {
            return Err(format!("Undefined Windows-1252 byte 0x{byte:02X}"));
        }
        out.push(
            char::from_u32(u32::from(code)).expect("CP1252 table contains Unicode scalar values"),
        );
    }
    Ok(out)
}

/// Read the exact old/new file data for the selected change or commit.
pub fn selected_file_pair(
    repo: &Repository,
    path: &str,
    staged: bool,
    commit_id: Option<&str>,
) -> Result<PreviewPair, String> {
    selected_file_pair_with_limit(repo, path, staged, commit_id, MAX_IMAGE_BYTES)
}

fn selected_file_pair_with_limit(
    repo: &Repository,
    path: &str,
    staged: bool,
    commit_id: Option<&str>,
    max_bytes: usize,
) -> Result<PreviewPair, String> {
    validate_repo_path(path)?;
    let key_base = repo_key(repo);
    if let Some(id) = commit_id {
        let oid = Oid::from_str(id).map_err(|e| e.to_string())?;
        let commit = repo.find_commit(oid).map_err(|e| e.to_string())?;
        let new_tree = commit.tree().map_err(|e| e.to_string())?;
        let old = if commit.parent_count() == 0 {
            None
        } else {
            Some(
                commit
                    .parent(0)
                    .map_err(|e| e.to_string())?
                    .tree()
                    .map_err(|e| e.to_string())?,
            )
        };
        let old = match old.as_ref() {
            Some(tree) => blob_from_tree(repo, tree, path, max_bytes)?,
            None => None,
        };
        let new = blob_from_tree(repo, &new_tree, path, max_bytes)?;
        return Ok(PreviewPair {
            old: old.map(|b| PreviewBlob {
                key: format!("{key_base}:{path}:commit:{}:old:{}", id, b.0),
                bytes: b.1,
            }),
            new: new.map(|b| PreviewBlob {
                key: format!("{key_base}:{path}:commit:{}:new:{}", id, b.0),
                bytes: b.1,
            }),
        });
    }
    if staged {
        let head = repo
            .head()
            .ok()
            .and_then(|h| h.peel_to_commit().ok())
            .and_then(|c| c.tree().ok());
        let old = match head.as_ref() {
            Some(tree) => blob_from_tree(repo, tree, path, max_bytes)?,
            None => None,
        };
        let new = blob_from_index(repo, path, max_bytes)?;
        Ok(PreviewPair {
            old: old.map(|b| PreviewBlob {
                key: format!(
                    "{key_base}:{path}:head:{}:old:{}",
                    head.as_ref()
                        .map(|t| t.id().to_string())
                        .unwrap_or_default(),
                    b.0
                ),
                bytes: b.1,
            }),
            new: new.map(|b| PreviewBlob {
                key: format!("{key_base}:{path}:index:new:{}", b.0),
                bytes: b.1,
            }),
        })
    } else {
        let old = blob_from_index(repo, path, max_bytes)?;
        let new = worktree_bytes(repo, path, max_bytes)?;
        Ok(PreviewPair {
            old: old.map(|b| PreviewBlob {
                key: format!("{key_base}:{path}:index:old:{}", b.0),
                bytes: b.1,
            }),
            new: new.map(|bytes| PreviewBlob {
                key: format!("{key_base}:{path}:worktree:new:{}", content_stamp(&bytes)),
                bytes,
            }),
        })
    }
}

fn validate_repo_path(path: &str) -> Result<(), String> {
    let p = Path::new(path);
    if path.is_empty()
        || p.is_absolute()
        || p.components().any(|c| {
            matches!(
                c,
                std::path::Component::ParentDir
                    | std::path::Component::RootDir
                    | std::path::Component::Prefix(_)
            )
        })
    {
        return Err("Preview path must be relative to the repository".into());
    }
    Ok(())
}

fn blob_from_tree(
    repo: &Repository,
    tree: &Tree<'_>,
    path: &str,
    max_bytes: usize,
) -> Result<Option<(String, Vec<u8>)>, String> {
    let entry = match tree.get_path(Path::new(path)) {
        Ok(entry) => entry,
        Err(e) if e.code() == git2::ErrorCode::NotFound => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    if entry.kind() != Some(ObjectType::Blob) || entry.filemode() == 0o120000 {
        return Ok(None);
    }
    let blob = repo.find_blob(entry.id()).map_err(|e| e.to_string())?;
    if blob.size() > max_bytes {
        return Err(format!(
            "File data exceeds the {} MiB preview limit ({} bytes)",
            max_bytes / (1024 * 1024),
            blob.size()
        ));
    }
    Ok(Some((entry.id().to_string(), blob.content().to_vec())))
}

fn blob_from_index(
    repo: &Repository,
    path: &str,
    max_bytes: usize,
) -> Result<Option<(String, Vec<u8>)>, String> {
    let index = repo.index().map_err(|e| e.to_string())?;
    let Some(entry) = index.get_path(Path::new(path), 0) else {
        return Ok(None);
    };
    if entry.mode == 0o120000 {
        return Ok(None);
    }
    let blob = repo.find_blob(entry.id).map_err(|e| e.to_string())?;
    if blob.size() > max_bytes {
        return Err(format!(
            "File data exceeds the {} MiB preview limit ({} bytes)",
            max_bytes / (1024 * 1024),
            blob.size()
        ));
    }
    Ok(Some((entry.id.to_string(), blob.content().to_vec())))
}

fn worktree_bytes(
    repo: &Repository,
    path: &str,
    max_bytes: usize,
) -> Result<Option<Vec<u8>>, String> {
    let Some(root) = repo.workdir() else {
        return Ok(None);
    };
    let full = root.join(path);
    let meta = match std::fs::symlink_metadata(&full) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    if meta.file_type().is_symlink() {
        return Err("Symbolic links are not opened as image previews".into());
    }
    if !meta.is_file() {
        return Ok(None);
    }
    if meta.len() > max_bytes as u64 {
        return Err(format!(
            "File data exceeds the {} MiB preview limit ({} bytes)",
            max_bytes / (1024 * 1024),
            meta.len()
        ));
    }
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    let resolved = full.canonicalize().map_err(|e| e.to_string())?;
    if !resolved.starts_with(&root) {
        return Err("Resolved preview path escapes the repository".into());
    }
    let file = std::fs::File::open(resolved).map_err(|e| e.to_string())?;
    let mut bytes = Vec::with_capacity(meta.len().min(max_bytes as u64) as usize);
    file.take(max_bytes as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > max_bytes {
        return Err(format!(
            "File data exceeds the {} MiB preview limit",
            max_bytes / (1024 * 1024)
        ));
    }
    Ok(Some(bytes))
}

fn repo_key(repo: &Repository) -> String {
    let path = repo.workdir().unwrap_or_else(|| repo.path());
    path.canonicalize()
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .into_owned()
}
fn content_stamp(bytes: &[u8]) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn is_image_candidate(path: Option<&str>, bytes: &[u8]) -> bool {
    image::guess_format(bytes).is_ok()
        || path.and_then(image::ImageFormat::from_extension).is_some()
}

fn decode_limited_image(bytes: &[u8]) -> Result<image::RgbaImage, String> {
    if bytes.len() > MAX_IMAGE_BYTES {
        return Err("Image exceeds the 16 MiB preview limit".into());
    }
    let format =
        image::guess_format(bytes).map_err(|e| format!("Unsupported or invalid image: {e}"))?;
    let dimensions = ImageReader::with_format(Cursor::new(bytes), format)
        .into_dimensions()
        .map_err(|e| format!("Could not read image dimensions: {e}"))?;
    let pixels = u64::from(dimensions.0)
        .checked_mul(u64::from(dimensions.1))
        .ok_or_else(|| "Image dimensions overflow".to_owned())?;
    if pixels > MAX_IMAGE_PIXELS {
        return Err(format!(
            "Image has {pixels} pixels; preview limit is {MAX_IMAGE_PIXELS}"
        ));
    }
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_IMAGE_PIXELS as u32);
    limits.max_image_height = Some(MAX_IMAGE_PIXELS as u32);
    limits.max_alloc = Some(MAX_IMAGE_DECODE_BYTES);
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    reader.limits(limits);
    reader
        .decode()
        .map(|image| image.to_rgba8())
        .map_err(|e| format!("Could not decode image: {e}"))
}

impl GritApp {
    fn cache_selected_pair(
        &mut self,
        repo: &Repository,
        path: &str,
        commit_id: Option<&str>,
        staged: bool,
        max_bytes: usize,
    ) {
        let pair_key = format!(
            "{}|{}|{}|{}|{}",
            repo_key(repo),
            path,
            commit_id.unwrap_or("working-tree"),
            staged,
            max_bytes
        );
        if self
            .preview_tools
            .pair_cache
            .as_ref()
            .is_none_or(|(cached_key, _)| cached_key != &pair_key)
        {
            let pair = selected_file_pair_with_limit(repo, path, staged, commit_id, max_bytes);
            self.preview_tools.pair_cache = Some((pair_key, pair));
        }
    }

    /// Render old/new image sides and return true when this selection is an image candidate.
    pub fn image_diff_preview(&mut self, ui: &mut egui::Ui) -> bool {
        let Some(repo) = self.repo() else {
            return false;
        };
        let (path, commit_id) = if self.tab == 1 {
            let Some(commit_id) = self.selected_commit.clone() else {
                return false;
            };
            let Some(path) = self.selected_commit_file.clone() else {
                return false;
            };
            (path, Some(commit_id))
        } else if self.tab == 0 {
            let Some(path) = self.selected_file.clone() else {
                return false;
            };
            (path, None)
        } else {
            return false;
        };
        // File extensions let us avoid opening any source bytes for ordinary text files.
        if !is_image_path(&path) {
            return false;
        }
        let staged = self.selected_staged;
        self.cache_selected_pair(&repo, &path, commit_id.as_deref(), staged, MAX_IMAGE_BYTES);
        let Some((_, pair_result)) = self.preview_tools.pair_cache.as_ref() else {
            return false;
        };
        let pair = match pair_result {
            Ok(pair) => pair,
            Err(error) => {
                ui.colored_label(egui::Color32::LIGHT_RED, error);
                return true;
            }
        };
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(&path).strong());
            ui.label(if commit_id.is_some() {
                "Commit image change"
            } else if staged {
                "HEAD ↔ Index"
            } else {
                "Index ↔ Worktree"
            });
        });
        let ctx = ui.ctx().clone();
        let cache = &mut self.preview_tools.texture_cache;
        ui.columns(2, |columns| {
            render_image_side(
                cache,
                &mut columns[0],
                &ctx,
                "Old",
                pair.old.as_ref().map(|blob| PreviewBlobRef {
                    key: &blob.key,
                    bytes: &blob.bytes,
                }),
                &path,
            );
            render_image_side(
                cache,
                &mut columns[1],
                &ctx,
                "New",
                pair.new.as_ref().map(|blob| PreviewBlobRef {
                    key: &blob.key,
                    bytes: &blob.bytes,
                }),
                &path,
            );
        });
        true
    }

    /// Show a strict, read-only, virtualized old/new text preview in the selected encoding.
    pub fn decoded_diff_preview(&mut self, ui: &mut egui::Ui) -> bool {
        let encoding = self.preview_tools.encoding;
        if encoding == TextEncoding::Utf8 {
            return false;
        }
        let Some(repo) = self.repo() else {
            return false;
        };
        let (path, commit_id) = if self.tab == 1 {
            let Some(commit_id) = self.selected_commit.clone() else {
                return false;
            };
            let Some(path) = self.selected_commit_file.clone() else {
                return false;
            };
            (path, Some(commit_id))
        } else if self.tab == 0 {
            let Some(path) = self.selected_file.clone() else {
                return false;
            };
            (path, None)
        } else {
            return false;
        };
        let staged = self.selected_staged;
        self.cache_selected_pair(&repo, &path, commit_id.as_deref(), staged, MAX_TEXT_BYTES);

        let PreviewTools {
            pair_cache,
            decoded_cache,
            ..
        } = &mut self.preview_tools;
        let Some((_, pair_result)) = pair_cache.as_ref() else {
            return false;
        };
        let pair = match pair_result {
            Ok(pair) => pair,
            Err(error) => {
                ui.heading(&path);
                ui.colored_label(egui::Color32::LIGHT_RED, error);
                ui.label("Decoded text preview limit: 4 MiB per side.");
                return true;
            }
        };
        if let Some(blob) = &pair.old {
            decoded_cache.ensure(decoded_text_key(blob, encoding), &blob.bytes, encoding);
        }
        if let Some(blob) = &pair.new {
            decoded_cache.ensure(decoded_text_key(blob, encoding), &blob.bytes, encoding);
        }
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(&path).strong());
            ui.label(format!(
                "Read-only · {} · line staging unavailable",
                encoding.label()
            ));
        });
        let old = pair
            .old
            .as_ref()
            .and_then(|blob| decoded_cache.texts.get(&decoded_text_key(blob, encoding)));
        let new = pair
            .new
            .as_ref()
            .and_then(|blob| decoded_cache.texts.get(&decoded_text_key(blob, encoding)));
        ui.columns(2, |columns| {
            render_decoded_side(&mut columns[0], "Old", old);
            render_decoded_side(&mut columns[1], "New", new);
        });
        true
    }

    /// Image-only helper for file-history panes; text fallback remains caller-owned.
    pub fn bytes_preview(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        key: &str,
        bytes: &[u8],
    ) -> bool {
        if !is_image_candidate(None, bytes) {
            return false;
        }
        render_image_side(
            &mut self.preview_tools.texture_cache,
            ui,
            ctx,
            "Preview",
            Some(PreviewBlobRef { key, bytes }),
            "",
        );
        true
    }
}

fn render_image_side(
    tools: &mut TextureCache,
    ui: &mut egui::Ui,
    ctx: &egui::Context,
    title: &str,
    blob: Option<PreviewBlobRef<'_>>,
    path: &str,
) {
    ui.heading(title);
    let Some(blob) = blob else {
        ui.label("No file on this side");
        return;
    };
    if !is_image_candidate(Some(path), blob.bytes) {
        ui.label("This side does not contain image data");
        return;
    }
    let key = format!("{}:{path}:{}", blob.key, content_stamp(blob.bytes));
    if let Some(cached) = tools.textures.get(&key) {
        match cached {
            Ok(texture) => {
                let max_size = ui.available_size();
                ui.add(egui::Image::new((texture.id(), texture.size_vec2())).max_size(max_size));
            }
            Err(error) => {
                ui.colored_label(egui::Color32::LIGHT_RED, error);
            }
        }
        return;
    }
    match tools.texture(ctx, key.clone(), blob.bytes) {
        Ok(texture) => {
            let max_size = ui.available_size();
            ui.add(egui::Image::new((texture.id(), texture.size_vec2())).max_size(max_size));
        }
        Err(error) => {
            tools.cached_error(key, error.clone());
            ui.colored_label(egui::Color32::LIGHT_RED, error);
        }
    }
}

fn decoded_text_key(blob: &PreviewBlob, encoding: TextEncoding) -> String {
    format!("{}:{encoding:?}", blob.key)
}

fn render_decoded_side(
    ui: &mut egui::Ui,
    title: &str,
    decoded: Option<&Result<DecodedText, String>>,
) {
    ui.heading(title);
    match decoded {
        None => {
            ui.label("No file on this side");
        }
        Some(Err(error)) => {
            ui.colored_label(egui::Color32::LIGHT_RED, error);
        }
        Some(Ok(decoded)) if decoded.text.is_empty() => {
            ui.label("Empty file");
        }
        Some(Ok(decoded)) => {
            let line_height = ui.text_style_height(&egui::TextStyle::Monospace);
            egui::ScrollArea::vertical()
                .id_salt(("decoded-diff", title))
                .show_rows(ui, line_height, decoded.line_starts.len(), |ui, rows| {
                    for row in rows {
                        let start = decoded.line_starts[row] as usize;
                        let mut end = decoded
                            .line_starts
                            .get(row + 1)
                            .map(|&offset| offset as usize)
                            .unwrap_or(decoded.text.len());
                        if end > start && decoded.text.as_bytes()[end - 1] == b'\n' {
                            end -= 1;
                        }
                        if end > start && decoded.text.as_bytes()[end - 1] == b'\r' {
                            end -= 1;
                        }
                        ui.monospace(format!("{:>6} {}", row + 1, &decoded.text[start..end]));
                    }
                });
        }
    }
}

fn is_image_path(path: &str) -> bool {
    Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .and_then(image::ImageFormat::from_extension)
        .is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, process::Command};

    #[test]
    fn encodings_round_trip_and_reject_invalid_utf16() {
        for encoding in [
            TextEncoding::Utf8,
            TextEncoding::Latin1,
            TextEncoding::Windows1252,
            TextEncoding::Utf16Le,
            TextEncoding::Utf16Be,
        ] {
            let text = match encoding {
                TextEncoding::Utf8 | TextEncoding::Utf16Le | TextEncoding::Utf16Be => "café € 😀",
                TextEncoding::Latin1 => "café",
                TextEncoding::Windows1252 => "café € —",
            };
            let bytes = encode_text(text, encoding, true).unwrap();
            assert_eq!(decode_text(&bytes, encoding).unwrap(), text);
        }
        let latin = encode_text("café", TextEncoding::Latin1, false).unwrap();
        assert_eq!(decode_text(&latin, TextEncoding::Latin1).unwrap(), "café");
        let cp = encode_text("€ — “x”", TextEncoding::Windows1252, false).unwrap();
        assert_eq!(
            decode_text(&cp, TextEncoding::Windows1252).unwrap(),
            "€ — “x”"
        );
        assert!(decode_text(&[0x00], TextEncoding::Utf16Le).is_err());
        assert!(decode_text(&[0x00, 0xd8], TextEncoding::Utf16Le).is_err());
        assert!(decode_text(&[0xff], TextEncoding::Utf8).is_err());
    }

    fn git(dir: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    }
    fn png(color: [u8; 4]) -> Vec<u8> {
        let image = image::RgbaImage::from_pixel(1, 1, image::Rgba(color));
        let mut bytes = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image)
            .write_to(&mut bytes, image::ImageFormat::Png)
            .unwrap();
        bytes.into_inner()
    }

    #[test]
    fn real_repo_preview_pairs_cover_add_delete_staged_and_history() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("grit-preview-{}-{nonce}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-b", "main"]);
        git(&dir, &["config", "user.name", "Tester"]);
        git(&dir, &["config", "user.email", "test@example.invalid"]);
        let old = png([255, 0, 0, 255]);
        let new = png([0, 255, 0, 255]);
        fs::write(dir.join("gone.png"), &old).unwrap();
        fs::write(dir.join("stay.png"), &old).unwrap();
        git(&dir, &["add", "."]);
        git(&dir, &["commit", "-m", "root"]);
        let root = git(&dir, &["rev-parse", "HEAD"]);
        fs::remove_file(dir.join("gone.png")).unwrap();
        git(&dir, &["add", "-u"]);
        let deleted =
            selected_file_pair(&Repository::open(&dir).unwrap(), "gone.png", true, None).unwrap();
        assert!(deleted.old.is_some());
        assert!(deleted.new.is_none());
        git(&dir, &["commit", "-m", "delete image", "--", "gone.png"]);
        let deletion_commit = git(&dir, &["rev-parse", "HEAD"]);
        let historical_delete = selected_file_pair(
            &Repository::open(&dir).unwrap(),
            "gone.png",
            false,
            Some(&deletion_commit),
        )
        .unwrap();
        assert_eq!(historical_delete.old.unwrap().bytes, old);
        assert!(historical_delete.new.is_none());
        fs::write(dir.join("added.png"), &new).unwrap();
        let added =
            selected_file_pair(&Repository::open(&dir).unwrap(), "added.png", false, None).unwrap();
        assert!(added.old.is_none());
        assert_eq!(added.new.unwrap().bytes, new);
        fs::write(dir.join("stay.png"), &new).unwrap();
        let staged_old =
            selected_file_pair(&Repository::open(&dir).unwrap(), "stay.png", false, None).unwrap();
        assert_eq!(staged_old.old.unwrap().bytes, old);
        git(&dir, &["add", "stay.png"]);
        let staged =
            selected_file_pair(&Repository::open(&dir).unwrap(), "stay.png", true, None).unwrap();
        assert_eq!(staged.old.unwrap().bytes, old);
        assert_eq!(staged.new.unwrap().bytes, new);
        git(&dir, &["commit", "-m", "update image"]);
        let update = git(&dir, &["rev-parse", "HEAD"]);
        let historic_update = selected_file_pair(
            &Repository::open(&dir).unwrap(),
            "stay.png",
            false,
            Some(&update),
        )
        .unwrap();
        assert_eq!(historic_update.old.unwrap().bytes, old);
        assert_eq!(historic_update.new.unwrap().bytes, new);
        let historic = selected_file_pair(
            &Repository::open(&dir).unwrap(),
            "stay.png",
            false,
            Some(&root),
        )
        .unwrap();
        assert!(historic.old.is_none());
        assert_eq!(historic.new.unwrap().bytes, old);
        assert!(
            selected_file_pair(
                &Repository::open(&dir).unwrap(),
                "../outside.png",
                false,
                None
            )
            .is_err()
        );
        fs::write(dir.join("large.txt"), vec![b'x'; MAX_TEXT_BYTES + 1]).unwrap();
        let limit_error = selected_file_pair_with_limit(
            &Repository::open(&dir).unwrap(),
            "large.txt",
            false,
            None,
            MAX_TEXT_BYTES,
        )
        .unwrap_err();
        assert!(limit_error.contains("4 MiB"));
        #[cfg(unix)]
        {
            let outside = dir.with_extension("outside.png");
            fs::write(&outside, png([1, 2, 3, 255])).unwrap();
            std::os::unix::fs::symlink(&outside, dir.join("linked.png")).unwrap();
            assert!(
                selected_file_pair(&Repository::open(&dir).unwrap(), "linked.png", false, None)
                    .is_err()
            );
            fs::remove_file(outside).unwrap();
        }
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn image_decode_enforces_pixel_and_byte_limits() {
        assert!(decode_limited_image(&vec![0; MAX_IMAGE_BYTES + 1]).is_err());
        assert_eq!(
            decode_limited_image(&png([12, 34, 56, 255]))
                .unwrap()
                .dimensions(),
            (1, 1)
        );
        let mut huge_header = png([12, 34, 56, 255]);
        huge_header[16..20].copy_from_slice(&4097_u32.to_be_bytes());
        huge_header[20..24].copy_from_slice(&4097_u32.to_be_bytes());
        let crc = png_crc32(&huge_header[12..29]);
        huge_header[29..33].copy_from_slice(&crc.to_be_bytes());
        let error = decode_limited_image(&huge_header).unwrap_err();
        assert!(error.contains("pixels"), "{error}");
    }

    fn png_crc32(bytes: &[u8]) -> u32 {
        let mut crc = !0_u32;
        for byte in bytes {
            crc ^= u32::from(*byte);
            for _ in 0..8 {
                crc = if crc & 1 == 1 {
                    (crc >> 1) ^ 0xedb8_8320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }
}
