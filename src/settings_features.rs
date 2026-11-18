use eframe::egui;
use std::{fs, io::Read, path::Path, sync::Arc};

const MAX_CUSTOM_FONT_BYTES: usize = 8 * 1024 * 1024;
const RECORD_COUNT: usize = 10;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ThemeMode {
    #[default]
    System,
    Dark,
    Light,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FontChoice {
    #[default]
    Proportional,
    Monospace,
    Custom,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AppLanguage {
    #[default]
    English,
    German,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ShortcutAction {
    OpenRepository,
    Refresh,
    ToggleFocus,
    Quit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ShortcutKey {
    O,
    R,
    M,
    Q,
    A,
    C,
    V,
    X,
    Z,
    Y,
    S,
    F,
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,
    Tab,
    Escape,
}

impl ShortcutKey {
    fn egui_key(self) -> egui::Key {
        match self {
            Self::O => egui::Key::O,
            Self::R => egui::Key::R,
            Self::M => egui::Key::M,
            Self::Q => egui::Key::Q,
            Self::A => egui::Key::A,
            Self::C => egui::Key::C,
            Self::V => egui::Key::V,
            Self::X => egui::Key::X,
            Self::Z => egui::Key::Z,
            Self::Y => egui::Key::Y,
            Self::S => egui::Key::S,
            Self::F => egui::Key::F,
            Self::F1 => egui::Key::F1,
            Self::F2 => egui::Key::F2,
            Self::F3 => egui::Key::F3,
            Self::F4 => egui::Key::F4,
            Self::F5 => egui::Key::F5,
            Self::F6 => egui::Key::F6,
            Self::F7 => egui::Key::F7,
            Self::F8 => egui::Key::F8,
            Self::F9 => egui::Key::F9,
            Self::F10 => egui::Key::F10,
            Self::F11 => egui::Key::F11,
            Self::F12 => egui::Key::F12,
            Self::Tab => egui::Key::Tab,
            Self::Escape => egui::Key::Escape,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::O => "O",
            Self::R => "R",
            Self::M => "M",
            Self::Q => "Q",
            Self::A => "A",
            Self::C => "C",
            Self::V => "V",
            Self::X => "X",
            Self::Z => "Z",
            Self::Y => "Y",
            Self::S => "S",
            Self::F => "F",
            Self::F1 => "F1",
            Self::F2 => "F2",
            Self::F3 => "F3",
            Self::F4 => "F4",
            Self::F5 => "F5",
            Self::F6 => "F6",
            Self::F7 => "F7",
            Self::F8 => "F8",
            Self::F9 => "F9",
            Self::F10 => "F10",
            Self::F11 => "F11",
            Self::F12 => "F12",
            Self::Tab => "Tab",
            Self::Escape => "Escape",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "O" => Self::O,
            "R" => Self::R,
            "M" => Self::M,
            "Q" => Self::Q,
            "A" => Self::A,
            "C" => Self::C,
            "V" => Self::V,
            "X" => Self::X,
            "Z" => Self::Z,
            "Y" => Self::Y,
            "S" => Self::S,
            "F" => Self::F,
            "F1" => Self::F1,
            "F2" => Self::F2,
            "F3" => Self::F3,
            "F4" => Self::F4,
            "F5" => Self::F5,
            "F6" => Self::F6,
            "F7" => Self::F7,
            "F8" => Self::F8,
            "F9" => Self::F9,
            "F10" => Self::F10,
            "F11" => Self::F11,
            "F12" => Self::F12,
            "Tab" => Self::Tab,
            "Escape" => Self::Escape,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ShortcutSpec {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub key: ShortcutKey,
}

impl ShortcutSpec {
    pub const fn ctrl(key: ShortcutKey) -> Self {
        Self {
            ctrl: true,
            alt: false,
            shift: false,
            key,
        }
    }

    pub fn keyboard_shortcut(self) -> egui::KeyboardShortcut {
        let mut modifiers = egui::Modifiers {
            alt: self.alt,
            shift: self.shift,
            ..Default::default()
        };
        if self.ctrl {
            modifiers.command = true;
            #[cfg(target_os = "macos")]
            {
                modifiers.mac_cmd = true;
            }
            #[cfg(not(target_os = "macos"))]
            {
                modifiers.ctrl = true;
            }
        }
        egui::KeyboardShortcut::new(modifiers, self.key.egui_key())
    }

    fn encode(self) -> String {
        format!(
            "{}:{}:{}:{}",
            u8::from(self.ctrl),
            u8::from(self.alt),
            u8::from(self.shift),
            self.key.label()
        )
    }

    fn parse(value: &str) -> Option<Self> {
        let mut parts = value.splitn(4, ':');
        let ctrl = parse_flag(parts.next()?)?;
        let alt = parse_flag(parts.next()?)?;
        let shift = parse_flag(parts.next()?)?;
        let key = ShortcutKey::parse(parts.next()?)?;
        Some(Self {
            ctrl,
            alt,
            shift,
            key,
        })
    }
}

#[derive(Clone, Debug)]
pub struct AppSettings {
    pub theme: ThemeMode,
    pub text_size: u8,
    pub font_choice: FontChoice,
    pub custom_font_path: String,
    pub language: AppLanguage,
    pub open_repository: ShortcutSpec,
    pub refresh: ShortcutSpec,
    pub toggle_focus: ShortcutSpec,
    pub quit: ShortcutSpec,
    pub font_error: Option<String>,
    pub shortcut_error: Option<String>,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            theme: ThemeMode::System,
            text_size: 13,
            font_choice: FontChoice::Proportional,
            custom_font_path: String::new(),
            language: AppLanguage::English,
            open_repository: ShortcutSpec::ctrl(ShortcutKey::O),
            refresh: ShortcutSpec::ctrl(ShortcutKey::R),
            toggle_focus: ShortcutSpec::ctrl(ShortcutKey::M),
            quit: ShortcutSpec::ctrl(ShortcutKey::Q),
            font_error: None,
            shortcut_error: None,
        }
    }
}

impl AppSettings {
    pub fn shortcut(&self, action: ShortcutAction) -> egui::KeyboardShortcut {
        let configured = match action {
            ShortcutAction::OpenRepository => self.open_repository,
            ShortcutAction::Refresh => self.refresh,
            ShortcutAction::ToggleFocus => self.toggle_focus,
            ShortcutAction::Quit => self.quit,
        };
        let shortcut = if validate_shortcuts(&self.shortcuts()).is_ok() {
            configured
        } else {
            default_shortcut(action)
        };
        shortcut.keyboard_shortcut()
    }

    /// Stable record list for eframe persistence; the custom font file itself is never embedded.
    pub fn encode_records(&self) -> Vec<String> {
        vec![
            "settings-v1".into(),
            match self.theme {
                ThemeMode::System => "system",
                ThemeMode::Dark => "dark",
                ThemeMode::Light => "light",
            }
            .into(),
            self.text_size.to_string(),
            match self.font_choice {
                FontChoice::Proportional => "proportional",
                FontChoice::Monospace => "monospace",
                FontChoice::Custom => "custom",
            }
            .into(),
            percent_encode(&self.custom_font_path),
            match self.language {
                AppLanguage::English => "en",
                AppLanguage::German => "de",
            }
            .into(),
            self.open_repository.encode(),
            self.refresh.encode(),
            self.toggle_focus.encode(),
            self.quit.encode(),
        ]
    }

    pub fn decode_records(records: &[String]) -> Result<Self, String> {
        if records.len() != RECORD_COUNT
            || records.first().map(String::as_str) != Some("settings-v1")
        {
            return Err("Unsupported or incomplete settings record".into());
        }
        let theme = match records[1].as_str() {
            "system" => ThemeMode::System,
            "dark" => ThemeMode::Dark,
            "light" => ThemeMode::Light,
            _ => return Err("Invalid theme preference".into()),
        };
        let text_size = records[2]
            .parse::<u8>()
            .map_err(|_| "Invalid text size".to_owned())?;
        if !(10..=24).contains(&text_size) {
            return Err("Text size must be between 10 and 24".into());
        }
        let font_choice = match records[3].as_str() {
            "proportional" => FontChoice::Proportional,
            "monospace" => FontChoice::Monospace,
            "custom" => FontChoice::Custom,
            _ => return Err("Invalid font choice".into()),
        };
        let custom_font_path = percent_decode(&records[4])?;
        let language = match records[5].as_str() {
            "en" => AppLanguage::English,
            "de" => AppLanguage::German,
            _ => return Err("Invalid language preference".into()),
        };
        let mut settings = Self {
            theme,
            text_size,
            font_choice,
            custom_font_path,
            language,
            open_repository: ShortcutSpec::parse(&records[6]).ok_or("Invalid open shortcut")?,
            refresh: ShortcutSpec::parse(&records[7]).ok_or("Invalid refresh shortcut")?,
            toggle_focus: ShortcutSpec::parse(&records[8]).ok_or("Invalid focus shortcut")?,
            quit: ShortcutSpec::parse(&records[9]).ok_or("Invalid quit shortcut")?,
            ..Default::default()
        };
        validate_shortcuts(&settings.shortcuts()).map_err(str::to_owned)?;
        settings.shortcut_error = None;
        Ok(settings)
    }

    pub fn settings_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let mut changed = false;
        ui.heading(translate(self.language, "settings"));
        ui.horizontal(|ui| {
            ui.label(translate(self.language, "theme"));
            egui::ComboBox::from_id_salt("settings_theme")
                .selected_text(match self.theme {
                    ThemeMode::System => "System",
                    ThemeMode::Dark => "Dark",
                    ThemeMode::Light => "Light",
                })
                .show_ui(ui, |ui| {
                    changed |= ui
                        .selectable_value(&mut self.theme, ThemeMode::System, "System")
                        .changed();
                    changed |= ui
                        .selectable_value(&mut self.theme, ThemeMode::Dark, "Dark")
                        .changed();
                    changed |= ui
                        .selectable_value(&mut self.theme, ThemeMode::Light, "Light")
                        .changed();
                });
        });
        ui.horizontal(|ui| {
            ui.label(translate(self.language, "text_size"));
            changed |= ui
                .add(egui::Slider::new(&mut self.text_size, 10..=24))
                .changed();
        });
        ui.horizontal(|ui| {
            ui.label(translate(self.language, "font"));
            egui::ComboBox::from_id_salt("settings_font")
                .selected_text(match self.font_choice {
                    FontChoice::Proportional => "Proportional",
                    FontChoice::Monospace => "Monospace",
                    FontChoice::Custom => "Custom file",
                })
                .show_ui(ui, |ui| {
                    changed |= ui
                        .selectable_value(
                            &mut self.font_choice,
                            FontChoice::Proportional,
                            "Proportional",
                        )
                        .changed();
                    changed |= ui
                        .selectable_value(&mut self.font_choice, FontChoice::Monospace, "Monospace")
                        .changed();
                    changed |= ui
                        .selectable_value(&mut self.font_choice, FontChoice::Custom, "Custom file")
                        .changed();
                });
        });
        if ui.button(translate(self.language, "choose_font")).clicked()
            && let Some(path) = rfd::FileDialog::new()
                .add_filter("TrueType / OpenType fonts", &["ttf", "otf"])
                .pick_file()
        {
            match read_custom_font_file(&path) {
                Ok(_) => {
                    self.custom_font_path = path.to_string_lossy().into_owned();
                    self.font_error = None;
                    self.font_choice = FontChoice::Custom;
                    changed = true;
                }
                Err(error) => self.font_error = Some(error),
            }
        }
        if !self.custom_font_path.is_empty() {
            ui.label(egui::RichText::new(&self.custom_font_path).small().weak());
        }
        if let Some(error) = &self.font_error {
            ui.colored_label(egui::Color32::LIGHT_RED, error);
        }
        ui.horizontal(|ui| {
            ui.label(translate(self.language, "language"));
            egui::ComboBox::from_id_salt("settings_language")
                .selected_text(match self.language {
                    AppLanguage::English => "English",
                    AppLanguage::German => "Deutsch",
                })
                .show_ui(ui, |ui| {
                    changed |= ui
                        .selectable_value(&mut self.language, AppLanguage::English, "English")
                        .changed();
                    changed |= ui
                        .selectable_value(&mut self.language, AppLanguage::German, "Deutsch")
                        .changed();
                });
        });
        ui.label(
            egui::RichText::new(translate(self.language, "language_scope"))
                .small()
                .weak(),
        );
        ui.separator();
        ui.label(translate(self.language, "shortcuts"));
        for (action, shortcut) in [
            (ShortcutAction::OpenRepository, &mut self.open_repository),
            (ShortcutAction::Refresh, &mut self.refresh),
            (ShortcutAction::ToggleFocus, &mut self.toggle_focus),
            (ShortcutAction::Quit, &mut self.quit),
        ] {
            changed |= shortcut_ui(ui, self.language, action, shortcut);
        }
        match validate_shortcuts(&self.shortcuts()) {
            Ok(()) => self.shortcut_error = None,
            Err(error) => self.shortcut_error = Some(error.into()),
        }
        if let Some(error) = &self.shortcut_error {
            ui.colored_label(egui::Color32::LIGHT_RED, error);
        }
        if changed && self.shortcut_error.is_none() {
            self.font_error = self.apply_settings(ctx).err();
        }
    }

    pub fn apply_settings(&mut self, ctx: &egui::Context) -> Result<(), String> {
        validate_shortcuts(&self.shortcuts()).map_err(str::to_owned)?;
        ctx.set_theme(match self.theme {
            ThemeMode::System => egui::ThemePreference::System,
            ThemeMode::Dark => egui::ThemePreference::Dark,
            ThemeMode::Light => egui::ThemePreference::Light,
        });
        let mut style = (*ctx.style()).clone();
        let dark = ctx.theme() == egui::Theme::Dark;
        style.visuals = if dark {
            egui::Visuals::dark()
        } else {
            egui::Visuals::light()
        };
        if dark {
            style.visuals.panel_fill = egui::Color32::from_rgb(29, 33, 40);
            style.visuals.window_fill = egui::Color32::from_rgb(35, 39, 47);
            style.visuals.extreme_bg_color = egui::Color32::from_rgb(25, 28, 34);
            style.visuals.faint_bg_color = egui::Color32::from_rgb(35, 39, 47);
            style.visuals.override_text_color = Some(egui::Color32::from_rgb(226, 231, 239));
        }
        let body_family = match self.font_choice {
            FontChoice::Monospace => egui::FontFamily::Monospace,
            FontChoice::Proportional | FontChoice::Custom => egui::FontFamily::Proportional,
        };
        let size = f32::from(self.text_size.clamp(10, 24));
        style.text_styles.insert(
            egui::TextStyle::Body,
            egui::FontId::new(size, body_family.clone()),
        );
        style.text_styles.insert(
            egui::TextStyle::Button,
            egui::FontId::new(size, body_family),
        );
        style.text_styles.insert(
            egui::TextStyle::Heading,
            egui::FontId::new(size + 5., egui::FontFamily::Proportional),
        );
        style.text_styles.insert(
            egui::TextStyle::Small,
            egui::FontId::new((size - 2.).max(8.), egui::FontFamily::Proportional),
        );
        style.text_styles.insert(
            egui::TextStyle::Monospace,
            egui::FontId::new(size, egui::FontFamily::Monospace),
        );
        ctx.set_style(style);

        let mut definitions = egui::FontDefinitions::default();
        if self.font_choice == FontChoice::Custom {
            let bytes = match read_custom_font_file(Path::new(&self.custom_font_path)) {
                Ok(bytes) => bytes,
                Err(error) => {
                    ctx.set_fonts(egui::FontDefinitions::default());
                    return Err(error);
                }
            };
            definitions.font_data.insert(
                "grit_custom_font".into(),
                Arc::new(egui::FontData::from_owned(bytes)),
            );
            for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
                definitions
                    .families
                    .entry(family)
                    .or_default()
                    .insert(0, "grit_custom_font".into());
            }
        }
        ctx.set_fonts(definitions);
        Ok(())
    }

    fn shortcuts(&self) -> [ShortcutSpec; 4] {
        [
            self.open_repository,
            self.refresh,
            self.toggle_focus,
            self.quit,
        ]
    }
}

fn default_shortcut(action: ShortcutAction) -> ShortcutSpec {
    match action {
        ShortcutAction::OpenRepository => ShortcutSpec::ctrl(ShortcutKey::O),
        ShortcutAction::Refresh => ShortcutSpec::ctrl(ShortcutKey::R),
        ShortcutAction::ToggleFocus => ShortcutSpec::ctrl(ShortcutKey::M),
        ShortcutAction::Quit => ShortcutSpec::ctrl(ShortcutKey::Q),
    }
}

fn shortcut_ui(
    ui: &mut egui::Ui,
    language: AppLanguage,
    action: ShortcutAction,
    shortcut: &mut ShortcutSpec,
) -> bool {
    let title = match (language, action) {
        (AppLanguage::English, ShortcutAction::OpenRepository) => "Open repository",
        (AppLanguage::German, ShortcutAction::OpenRepository) => "Repository öffnen",
        (AppLanguage::English, ShortcutAction::Refresh) => "Refresh",
        (AppLanguage::German, ShortcutAction::Refresh) => "Aktualisieren",
        (AppLanguage::English, ShortcutAction::ToggleFocus) => "Focus mode",
        (AppLanguage::German, ShortcutAction::ToggleFocus) => "Fokusmodus",
        (AppLanguage::English, ShortcutAction::Quit) => "Quit",
        (AppLanguage::German, ShortcutAction::Quit) => "Beenden",
    };
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label(title);
        changed |= ui.checkbox(&mut shortcut.ctrl, "Ctrl").changed();
        changed |= ui.checkbox(&mut shortcut.alt, "Alt").changed();
        changed |= ui.checkbox(&mut shortcut.shift, "Shift").changed();
        egui::ComboBox::from_id_salt(format!("shortcut_{action:?}"))
            .selected_text(shortcut.key.label())
            .show_ui(ui, |ui| {
                for key in allowed_keys() {
                    changed |= ui
                        .selectable_value(&mut shortcut.key, key, key.label())
                        .changed();
                }
            });
    });
    changed
}

fn allowed_keys() -> [ShortcutKey; 18] {
    [
        ShortcutKey::O,
        ShortcutKey::R,
        ShortcutKey::M,
        ShortcutKey::Q,
        ShortcutKey::F1,
        ShortcutKey::F2,
        ShortcutKey::F3,
        ShortcutKey::F4,
        ShortcutKey::F5,
        ShortcutKey::F6,
        ShortcutKey::F7,
        ShortcutKey::F8,
        ShortcutKey::F9,
        ShortcutKey::F10,
        ShortcutKey::F11,
        ShortcutKey::F12,
        ShortcutKey::Tab,
        ShortcutKey::Escape,
    ]
}

pub fn validate_shortcuts(shortcuts: &[ShortcutSpec]) -> Result<(), &'static str> {
    for shortcut in shortcuts {
        if matches!(shortcut.key, ShortcutKey::Tab | ShortcutKey::Escape) {
            return Err("Tab and Escape are reserved navigation or system keys");
        }
        if !allowed_keys().contains(&shortcut.key) && !reserved_edit_key(shortcut.key) {
            return Err("Shortcut key is not supported");
        }
        if is_letter_key(shortcut.key) && !shortcut.ctrl && !shortcut.alt && !shortcut.shift {
            return Err("Letter shortcuts require at least one modifier");
        }
        if shortcut.ctrl && reserved_edit_key(shortcut.key) {
            return Err("Shortcut conflicts with a text-editing shortcut");
        }
        if shortcut.alt && shortcut.key == ShortcutKey::F4 {
            return Err("Alt+F4 is reserved by the operating system");
        }
    }
    for (index, shortcut) in shortcuts.iter().enumerate() {
        if shortcuts[index + 1..].contains(shortcut) {
            return Err("Shortcuts must be unique");
        }
    }
    Ok(())
}

fn reserved_edit_key(key: ShortcutKey) -> bool {
    matches!(
        key,
        ShortcutKey::A
            | ShortcutKey::C
            | ShortcutKey::V
            | ShortcutKey::X
            | ShortcutKey::Z
            | ShortcutKey::Y
            | ShortcutKey::S
            | ShortcutKey::F
    )
}

fn is_letter_key(key: ShortcutKey) -> bool {
    matches!(
        key,
        ShortcutKey::O
            | ShortcutKey::R
            | ShortcutKey::M
            | ShortcutKey::Q
            | ShortcutKey::A
            | ShortcutKey::C
            | ShortcutKey::V
            | ShortcutKey::X
            | ShortcutKey::Z
            | ShortcutKey::Y
            | ShortcutKey::S
            | ShortcutKey::F
    )
}

pub fn read_custom_font_file(path: &Path) -> Result<Vec<u8>, String> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if ext != "ttf" && ext != "otf" {
        return Err("Choose a .ttf or .otf font file".into());
    }
    let metadata = fs::metadata(path).map_err(|e| format!("Could not read font metadata: {e}"))?;
    if metadata.len() > MAX_CUSTOM_FONT_BYTES as u64 {
        return Err("Custom fonts are limited to 8 MiB".into());
    }
    let file = fs::File::open(path).map_err(|e| format!("Could not open font: {e}"))?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(MAX_CUSTOM_FONT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("Could not read font: {e}"))?;
    validate_custom_font_bytes(path, &bytes)?;
    Ok(bytes)
}

fn validate_custom_font_bytes(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if ext != "ttf" && ext != "otf" {
        return Err("Choose a .ttf or .otf font file".into());
    }
    if bytes.len() > MAX_CUSTOM_FONT_BYTES {
        return Err("Custom fonts are limited to 8 MiB".into());
    }
    if bytes.len() < 4 || !matches!(&bytes[..4], [0, 1, 0, 0] | b"OTTO" | b"true" | b"typ1") {
        return Err("Font file does not have a recognized TrueType/OpenType signature".into());
    }
    if bytes.len() < 12 {
        return Err("Font file has a truncated table directory".into());
    }
    let table_count = usize::from(u16::from_be_bytes([bytes[4], bytes[5]]));
    let directory_end = 12_usize
        .checked_add(
            table_count
                .checked_mul(16)
                .ok_or("Font table directory is too large")?,
        )
        .ok_or("Font table directory is too large")?;
    if table_count == 0 || directory_end > bytes.len() {
        return Err("Font file has a truncated table directory".into());
    }
    for table in bytes[12..directory_end].as_chunks::<16>().0 {
        let offset = u32::from_be_bytes([table[8], table[9], table[10], table[11]]) as usize;
        let length = u32::from_be_bytes([table[12], table[13], table[14], table[15]]) as usize;
        if offset
            .checked_add(length)
            .is_none_or(|end| end > bytes.len())
        {
            return Err("Font table points outside the file".into());
        }
    }
    ttf_parser::Face::parse(bytes, 0)
        .map_err(|error| format!("Could not parse font tables: {error:?}"))?;
    Ok(())
}

pub fn translate(language: AppLanguage, key: &str) -> &'static str {
    match (language, key) {
        (AppLanguage::German, "changes") => "Änderungen",
        (AppLanguage::German, "history") => "Verlauf",
        (AppLanguage::German, "branches") => "Branches",
        (AppLanguage::German, "remotes") => "Remotes",
        (AppLanguage::German, "files") => "Dateien",
        (AppLanguage::German, "settings") => "Einstellungen",
        (AppLanguage::German, "theme") => "Design",
        (AppLanguage::German, "text_size") => "Textgröße",
        (AppLanguage::German, "font") => "Schrift",
        (AppLanguage::German, "choose_font") => "TTF/OTF-Datei wählen…",
        (AppLanguage::German, "language") => "Sprache",
        (AppLanguage::German, "language_scope") => {
            "Übersetzt sind derzeit nur Kernnavigation und Einstellungen."
        }
        (AppLanguage::German, "shortcuts") => "Tastenkürzel",
        (_, "changes") => "Changes",
        (_, "history") => "History",
        (_, "branches") => "Branches",
        (_, "remotes") => "Remotes",
        (_, "files") => "Files",
        (_, "settings") => "Settings",
        (_, "theme") => "Theme",
        (_, "text_size") => "Text size",
        (_, "font") => "Font",
        (_, "choose_font") => "Choose TTF/OTF file…",
        (_, "language") => "Language",
        (_, "language_scope") => "Only core navigation and settings are translated so far.",
        (_, "shortcuts") => "Keyboard shortcuts",
        _ => "",
    }
}

fn percent_encode(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'\\' | b':' | b'.' | b'_' | b'-')
        {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

fn parse_flag(value: &str) -> Option<bool> {
    match value {
        "0" => Some(false),
        "1" => Some(true),
        _ => None,
    }
}

fn percent_decode(value: &str) -> Result<String, String> {
    let mut bytes = Vec::new();
    let input = value.as_bytes();
    let mut index = 0;
    while index < input.len() {
        if input[index] == b'%' {
            if index + 2 >= input.len() {
                return Err("Invalid escaped settings value".into());
            }
            let hex =
                std::str::from_utf8(&input[index + 1..index + 3]).map_err(|e| e.to_string())?;
            bytes.push(
                u8::from_str_radix(hex, 16)
                    .map_err(|_| "Invalid escaped settings value".to_owned())?,
            );
            index += 3;
        } else {
            bytes.push(input[index]);
            index += 1;
        }
    }
    String::from_utf8(bytes).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_records_round_trip_and_reject_invalid_preferences() {
        let settings = AppSettings {
            theme: ThemeMode::Light,
            text_size: 21,
            font_choice: FontChoice::Custom,
            custom_font_path: "/tmp/Schrift mit ü.ttf".into(),
            language: AppLanguage::German,
            toggle_focus: ShortcutSpec {
                ctrl: true,
                alt: true,
                shift: false,
                key: ShortcutKey::F6,
            },
            ..Default::default()
        };
        let records = settings.encode_records();
        let restored = AppSettings::decode_records(&records).unwrap();
        assert_eq!(restored.theme, ThemeMode::Light);
        assert_eq!(restored.text_size, 21);
        assert_eq!(restored.custom_font_path, settings.custom_font_path);
        assert_eq!(restored.language, AppLanguage::German);
        assert_eq!(restored.toggle_focus, settings.toggle_focus);

        let mut invalid = records;
        invalid[2] = "99".into();
        assert!(AppSettings::decode_records(&invalid).is_err());
        let mut duplicate = AppSettings::default().encode_records();
        duplicate[7] = duplicate[6].clone();
        assert!(AppSettings::decode_records(&duplicate).is_err());
    }

    #[test]
    fn shortcuts_reject_duplicates_and_reserved_editing_keys() {
        let mut shortcuts = AppSettings::default().shortcuts();
        shortcuts[1] = shortcuts[0];
        assert_eq!(
            validate_shortcuts(&shortcuts),
            Err("Shortcuts must be unique")
        );
        let reserved = ShortcutSpec::ctrl(ShortcutKey::C);
        assert_eq!(
            validate_shortcuts(&[reserved]),
            Err("Shortcut conflicts with a text-editing shortcut")
        );
        assert_eq!(
            validate_shortcuts(&[ShortcutSpec {
                ctrl: false,
                alt: false,
                shift: false,
                key: ShortcutKey::O
            }]),
            Err("Letter shortcuts require at least one modifier")
        );
        assert!(
            validate_shortcuts(&[ShortcutSpec {
                ctrl: false,
                alt: true,
                shift: false,
                key: ShortcutKey::O
            }])
            .is_ok()
        );
        assert_eq!(
            validate_shortcuts(&[ShortcutSpec::ctrl(ShortcutKey::Tab)]),
            Err("Tab and Escape are reserved navigation or system keys")
        );
        assert_eq!(
            validate_shortcuts(&[ShortcutSpec {
                ctrl: false,
                alt: true,
                shift: false,
                key: ShortcutKey::F4
            }]),
            Err("Alt+F4 is reserved by the operating system")
        );
    }

    #[test]
    fn custom_font_validation_checks_signature_and_size() {
        let mut malformed_sfnt = vec![0; 32];
        malformed_sfnt[..4].copy_from_slice(&[0, 1, 0, 0]);
        malformed_sfnt[4..6].copy_from_slice(&1_u16.to_be_bytes());
        malformed_sfnt[12..16].copy_from_slice(b"head");
        malformed_sfnt[20..24].copy_from_slice(&28_u32.to_be_bytes());
        malformed_sfnt[24..28].copy_from_slice(&4_u32.to_be_bytes());
        assert!(validate_custom_font_bytes(Path::new("font.ttf"), &malformed_sfnt).is_err());
        assert!(validate_custom_font_bytes(Path::new("font.ttf"), b"bad!").is_err());
        assert!(
            validate_custom_font_bytes(Path::new("font.otf"), &vec![0; MAX_CUSTOM_FONT_BYTES + 1])
                .is_err()
        );
        assert!(validate_custom_font_bytes(Path::new("font.woff"), b"wOFF").is_err());
    }
}
