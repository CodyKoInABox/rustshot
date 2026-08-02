//! Persistent user configuration.
//!
//! Configuration is stored as TOML in the platform's per-user configuration
//! directory. Saves use a temporary file in the same directory followed by a
//! rename so readers never observe a partially written configuration.

use std::{
    fmt,
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    str::FromStr,
    sync::atomic::{AtomicU64, Ordering},
};

use directories::{ProjectDirs, UserDirs};
use global_hotkey::hotkey::HotKey;
use serde::{de, Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;

/// Name of Rustshot's settings file.
pub const CONFIG_FILE_NAME: &str = "config.toml";

/// Settings schema written and understood by this Rustshot release.
pub const CURRENT_CONFIG_VERSION: u32 = 1;

/// Default shortcut for capturing the configured monitor scope immediately.
pub const DEFAULT_FULLSCREEN_SHORTCUT: &str = "control+shift+F11";

/// Default shortcut for opening the interactive region selector.
pub const DEFAULT_REGION_SHORTCUT: &str = "control+shift+F10";

const DEFAULT_JPEG_QUALITY: u8 = 90;
const TEMP_FILE_ATTEMPTS: u8 = 32;
static TEMP_FILE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// All persistent Rustshot settings.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Version of the persisted settings schema.
    pub config_version: u32,
    /// Captures [`monitor_scope`](Self::monitor_scope) and saves it immediately.
    pub fullscreen_shortcut: Shortcut,
    /// Opens the interactive area-selection overlay.
    pub region_shortcut: Shortcut,
    /// Directory used by the immediate full-screen capture workflow.
    pub autosave_directory: PathBuf,
    /// File format used when encoding screenshots.
    pub image_format: ScreenshotFormat,
    /// JPEG encoder quality in the inclusive range `1..=100`.
    pub jpeg_quality: u8,
    /// PNG encoder compression preference.
    pub png_compression: PngCompression,
    /// Which display area a full-screen capture includes.
    pub monitor_scope: MonitorScope,
    /// Whether the mouse cursor is composited into captures.
    pub include_cursor: bool,
    /// Copy a selected region immediately instead of opening the editor.
    pub region_auto_copy: bool,
    /// Save a selected region immediately instead of opening the editor.
    pub region_autosave: bool,
    /// Tool restored when the next region editor opens.
    pub last_editor_tool: EditorTool,
    /// Opaque annotation color restored by the region editor.
    pub editor_color: RgbColor,
    /// Index into the editor's compact stroke-width table.
    pub editor_stroke_width: u8,
}

impl Config {
    /// Loads settings from the standard per-user configuration path.
    ///
    /// A missing file is not an error; the default configuration is returned.
    pub fn load() -> Result<Self, ConfigError> {
        Self::load_from(config_path()?)
    }

    /// Loads settings from `path`.
    ///
    /// A missing file is not an error; the default configuration is returned.
    pub fn load_from(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let path = path.as_ref();
        let serialized = match fs::read_to_string(path) {
            Ok(serialized) => serialized,
            Err(source) if source.kind() == io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(source) => {
                return Err(ConfigError::Io {
                    operation: "read",
                    path: path.to_path_buf(),
                    source,
                });
            }
        };

        let config =
            toml::from_str::<Self>(&serialized).map_err(|source| ConfigError::Deserialize {
                path: path.to_path_buf(),
                source,
            })?;
        config.validate()?;
        Ok(config)
    }

    /// Atomically saves settings to the standard per-user configuration path.
    pub fn save(&self) -> Result<(), ConfigError> {
        self.save_to(config_path()?)
    }

    /// Atomically saves settings to `path`.
    ///
    /// The TOML is fully written and synchronized to a temporary file beside
    /// the destination before that file replaces the previous configuration.
    pub fn save_to(&self, path: impl AsRef<Path>) -> Result<(), ConfigError> {
        self.validate()?;
        let path = path.as_ref();
        let serialized = toml::to_string_pretty(self).map_err(ConfigError::Serialize)?;
        let parent = non_empty_parent(path);

        fs::create_dir_all(parent).map_err(|source| ConfigError::Io {
            operation: "create configuration directory",
            path: parent.to_path_buf(),
            source,
        })?;

        let (temporary_path, mut temporary_file) = create_temporary_file(path)?;
        let write_result = (|| {
            temporary_file
                .write_all(serialized.as_bytes())
                .map_err(|source| ConfigError::Io {
                    operation: "write temporary configuration",
                    path: temporary_path.clone(),
                    source,
                })?;
            temporary_file.sync_all().map_err(|source| ConfigError::Io {
                operation: "synchronize temporary configuration",
                path: temporary_path.clone(),
                source,
            })
        })();

        if let Err(error) = write_result {
            drop(temporary_file);
            let _ = fs::remove_file(&temporary_path);
            return Err(error);
        }

        drop(temporary_file);
        if let Err(source) = fs::rename(&temporary_path, path) {
            let _ = fs::remove_file(&temporary_path);
            return Err(ConfigError::Io {
                operation: "replace configuration",
                path: path.to_path_buf(),
                source,
            });
        }

        Ok(())
    }

    /// Checks all invariants that must hold before settings can be used.
    pub fn validate(&self) -> Result<(), ConfigValidationError> {
        if self.config_version != CURRENT_CONFIG_VERSION {
            return Err(ConfigValidationError::UnsupportedConfigVersion {
                found: self.config_version,
                supported: CURRENT_CONFIG_VERSION,
            });
        }

        let fullscreen = self.fullscreen_shortcut.as_hotkey().map_err(|source| {
            ConfigValidationError::InvalidShortcut {
                binding: ShortcutBinding::Fullscreen,
                shortcut: self.fullscreen_shortcut.as_str().to_owned(),
                reason: source.to_string(),
            }
        })?;
        let region = self.region_shortcut.as_hotkey().map_err(|source| {
            ConfigValidationError::InvalidShortcut {
                binding: ShortcutBinding::Region,
                shortcut: self.region_shortcut.as_str().to_owned(),
                reason: source.to_string(),
            }
        })?;

        if fullscreen == region {
            return Err(ConfigValidationError::DuplicateShortcuts {
                shortcut: self.fullscreen_shortcut.as_str().to_owned(),
            });
        }

        if !(1..=100).contains(&self.jpeg_quality) {
            return Err(ConfigValidationError::InvalidJpegQuality(self.jpeg_quality));
        }

        if self.autosave_directory.as_os_str().is_empty() {
            return Err(ConfigValidationError::EmptyAutosaveDirectory);
        }
        if self.autosave_directory.is_file() {
            return Err(ConfigValidationError::AutosaveDirectoryIsFile(
                self.autosave_directory.clone(),
            ));
        }

        if self.editor_stroke_width > 3 {
            return Err(ConfigValidationError::InvalidEditorStrokeWidth(
                self.editor_stroke_width,
            ));
        }

        Ok(())
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            config_version: CURRENT_CONFIG_VERSION,
            fullscreen_shortcut: default_shortcut(DEFAULT_FULLSCREEN_SHORTCUT),
            region_shortcut: default_shortcut(DEFAULT_REGION_SHORTCUT),
            autosave_directory: default_autosave_directory(),
            image_format: ScreenshotFormat::default(),
            jpeg_quality: DEFAULT_JPEG_QUALITY,
            png_compression: PngCompression::default(),
            monitor_scope: MonitorScope::default(),
            include_cursor: true,
            region_auto_copy: false,
            region_autosave: false,
            last_editor_tool: EditorTool::default(),
            editor_color: RgbColor::default(),
            editor_stroke_width: 1,
        }
    }
}

/// A canonical, validated global keyboard shortcut.
///
/// Modifiers precede the physical key, for example `control+shift+KeyS`.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Shortcut(String);

impl Shortcut {
    /// Parses and canonicalizes a shortcut supported by `global-hotkey`.
    pub fn new(shortcut: impl AsRef<str>) -> Result<Self, ShortcutError> {
        let input = shortcut.as_ref().trim();
        let hotkey = input
            .parse::<HotKey>()
            .map_err(|source| ShortcutError::Invalid {
                shortcut: input.to_owned(),
                reason: source.to_string(),
            })?;
        Ok(Self(hotkey.into_string()))
    }

    /// Returns the canonical serialized form.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Converts the setting to the registration type used by `global-hotkey`.
    pub fn as_hotkey(&self) -> Result<HotKey, ShortcutError> {
        self.0
            .parse::<HotKey>()
            .map_err(|source| ShortcutError::Invalid {
                shortcut: self.0.clone(),
                reason: source.to_string(),
            })
    }
}

impl fmt::Display for Shortcut {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for Shortcut {
    type Err = ShortcutError;

    fn from_str(shortcut: &str) -> Result<Self, Self::Err> {
        Self::new(shortcut)
    }
}

impl TryFrom<&str> for Shortcut {
    type Error = ShortcutError;

    fn try_from(shortcut: &str) -> Result<Self, Self::Error> {
        Self::new(shortcut)
    }
}

impl TryFrom<String> for Shortcut {
    type Error = ShortcutError;

    fn try_from(shortcut: String) -> Result<Self, Self::Error> {
        Self::new(shortcut)
    }
}

impl From<HotKey> for Shortcut {
    fn from(hotkey: HotKey) -> Self {
        Self(hotkey.into_string())
    }
}

impl Serialize for Shortcut {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for Shortcut {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let shortcut = String::deserialize(deserializer)?;
        Self::new(shortcut).map_err(de::Error::custom)
    }
}

/// Screenshot file format.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ScreenshotFormat {
    /// Lossless Portable Network Graphics.
    #[default]
    Png,
    /// Lossy Joint Photographic Experts Group format.
    Jpeg,
}

impl ScreenshotFormat {
    /// Conventional filename extension without a leading dot.
    #[must_use]
    pub const fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpg",
        }
    }
}

/// PNG encoder compression preference.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PngCompression {
    /// Prioritize encoding speed and use less CPU.
    Fast,
    /// Balance file size and encoding speed.
    #[default]
    Default,
    /// Prioritize a smaller file at the cost of encoding time.
    Best,
}

/// Display area included by a full-screen capture.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MonitorScope {
    /// Capture only the display containing the mouse pointer.
    #[default]
    CursorMonitor,
    /// Capture the bounding rectangle containing every active display.
    VirtualDesktop,
}

/// Editor tool persisted between region-capture sessions.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EditorTool {
    Select,
    #[default]
    Pen,
    Highlighter,
    Line,
    Arrow,
    Rectangle,
    Ellipse,
    Text,
    Callout,
    Redact,
    Pixelate,
    Eraser,
    Crop,
    Eyedropper,
}

/// Compact serializable RGB color used for editor preferences.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RgbColor {
    pub red: u8,
    pub green: u8,
    pub blue: u8,
}

impl RgbColor {
    #[must_use]
    pub const fn new(red: u8, green: u8, blue: u8) -> Self {
        Self { red, green, blue }
    }
}

impl Default for RgbColor {
    fn default() -> Self {
        Self::new(255, 64, 64)
    }
}

/// Identifies one of the two independently configurable capture shortcuts.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShortcutBinding {
    /// Immediate capture of the configured monitor scope.
    Fullscreen,
    /// Interactive region selection.
    Region,
}

impl fmt::Display for ShortcutBinding {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Fullscreen => formatter.write_str("fullscreen"),
            Self::Region => formatter.write_str("region"),
        }
    }
}

/// Error returned when parsing a global shortcut.
#[derive(Debug, Error)]
pub enum ShortcutError {
    /// The shortcut cannot be represented by the global hotkey library.
    #[error("invalid shortcut `{shortcut}`: {reason}")]
    Invalid {
        /// User-provided shortcut.
        shortcut: String,
        /// Parser explanation.
        reason: String,
    },
}

/// A semantic error in an otherwise readable settings file.
#[derive(Debug, Error, Eq, PartialEq)]
pub enum ConfigValidationError {
    /// The settings were written by an unsupported schema version.
    #[error(
        "configuration schema {found} is not supported; this release supports schema {supported}"
    )]
    UnsupportedConfigVersion {
        /// Schema found in the settings file.
        found: u32,
        /// Schema understood by this release.
        supported: u32,
    },
    /// A capture shortcut could not be parsed.
    #[error("invalid {binding} shortcut `{shortcut}`: {reason}")]
    InvalidShortcut {
        /// Binding containing the invalid value.
        binding: ShortcutBinding,
        /// Invalid serialized shortcut.
        shortcut: String,
        /// Parser explanation.
        reason: String,
    },
    /// Both capture actions would be triggered by the same key combination.
    #[error("fullscreen and region shortcuts must differ (both are `{shortcut}`)")]
    DuplicateShortcuts {
        /// Canonical duplicated shortcut.
        shortcut: String,
    },
    /// JPEG quality falls outside the encoder's supported user range.
    #[error("JPEG quality must be between 1 and 100, got {0}")]
    InvalidJpegQuality(u8),
    /// No directory was configured for immediate captures.
    #[error("autosave directory must not be empty")]
    EmptyAutosaveDirectory,
    /// The configured autosave destination is an existing regular file.
    #[error("autosave directory points to an existing file: `{}`", .0.display())]
    AutosaveDirectoryIsFile(PathBuf),
    /// The persisted editor width does not index the fixed width table.
    #[error("editor stroke-width index must be between 0 and 3, got {0}")]
    InvalidEditorStrokeWidth(u8),
}

/// Failure while resolving, loading, validating, or saving settings.
#[derive(Debug, Error)]
pub enum ConfigError {
    /// No per-user project directory is available on this system.
    #[error("could not determine the per-user Rustshot configuration directory")]
    ConfigurationDirectoryUnavailable,
    /// A filesystem operation failed.
    #[error("failed to {operation} at `{}`: {source}", path.display())]
    Io {
        /// Operation that failed.
        operation: &'static str,
        /// Relevant filesystem path.
        path: PathBuf,
        /// Operating-system error.
        #[source]
        source: io::Error,
    },
    /// TOML could not be decoded into settings.
    #[error("failed to parse configuration at `{}`: {source}", path.display())]
    Deserialize {
        /// Configuration file path.
        path: PathBuf,
        /// TOML decoding error.
        #[source]
        source: toml::de::Error,
    },
    /// Settings could not be encoded as TOML.
    #[error("failed to serialize configuration: {0}")]
    Serialize(#[source] toml::ser::Error),
    /// Settings violate a semantic invariant.
    #[error(transparent)]
    Validation(#[from] ConfigValidationError),
}

/// Returns the platform-appropriate per-user Rustshot settings path.
pub fn config_path() -> Result<PathBuf, ConfigError> {
    ProjectDirs::from("io.github", "CodyKoInABox", "Rustshot")
        .map(|directories| directories.config_dir().join(CONFIG_FILE_NAME))
        .ok_or(ConfigError::ConfigurationDirectoryUnavailable)
}

fn default_autosave_directory() -> PathBuf {
    if let Some(pictures) = UserDirs::new().and_then(|directories| {
        directories
            .picture_dir()
            .map(|directory| directory.to_path_buf())
    }) {
        return pictures.join("Rustshot");
    }

    ProjectDirs::from("io.github", "CodyKoInABox", "Rustshot")
        .map(|directories| directories.data_local_dir().join("captures"))
        .unwrap_or_else(|| PathBuf::from("Rustshot"))
}

fn default_shortcut(shortcut: &str) -> Shortcut {
    Shortcut::new(shortcut).expect("the built-in Rustshot shortcut must be valid")
}

fn non_empty_parent(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

fn create_temporary_file(destination: &Path) -> Result<(PathBuf, fs::File), ConfigError> {
    let parent = non_empty_parent(destination);
    let file_name = destination
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(CONFIG_FILE_NAME);

    for _ in 0..TEMP_FILE_ATTEMPTS {
        let sequence = TEMP_FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let temporary_path = parent.join(format!(
            ".{file_name}.{}.{}.tmp",
            std::process::id(),
            sequence
        ));
        match OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary_path)
        {
            Ok(file) => return Ok((temporary_path, file)),
            Err(source) if source.kind() == io::ErrorKind::AlreadyExists => {}
            Err(source) => {
                return Err(ConfigError::Io {
                    operation: "create temporary configuration",
                    path: temporary_path,
                    source,
                });
            }
        }
    }

    Err(ConfigError::Io {
        operation: "create a unique temporary configuration",
        path: parent.to_path_buf(),
        source: io::Error::new(
            io::ErrorKind::AlreadyExists,
            "temporary configuration filename attempts exhausted",
        ),
    })
}

#[cfg(test)]
#[allow(clippy::field_reassign_with_default)]
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
        sync::atomic::{AtomicU64, Ordering},
    };

    use super::{
        Config, ConfigError, ConfigValidationError, EditorTool, MonitorScope, PngCompression,
        RgbColor, ScreenshotFormat, Shortcut,
    };

    static TEST_DIRECTORY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let sequence = TEST_DIRECTORY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "rustshot-config-test-{}-{sequence}",
                std::process::id()
            ));
            fs::create_dir(&path).expect("test directory should be created");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn shortcut_parsing_canonicalizes_aliases_and_whitespace() {
        let shortcut = Shortcut::new(" Ctrl + Shift + KeyS ").expect("shortcut should be valid");
        let equivalent = Shortcut::new("shift+control+KeyS").expect("shortcut should be valid");

        assert_eq!(shortcut, equivalent);
        assert_eq!(
            shortcut.as_hotkey().expect("shortcut remains valid"),
            equivalent.as_hotkey().expect("shortcut remains valid")
        );
    }

    #[test]
    fn invalid_shortcut_is_rejected() {
        let error = Shortcut::new("control+definitely-not-a-key")
            .expect_err("unknown key should be rejected");

        assert!(error.to_string().contains("invalid shortcut"));
    }

    #[test]
    fn duplicate_shortcuts_are_rejected_after_canonicalization() {
        let mut config = Config::default();
        config.fullscreen_shortcut =
            Shortcut::new("control+shift+KeyS").expect("shortcut should be valid");
        config.region_shortcut =
            Shortcut::new("shift+ctrl+KeyS").expect("shortcut should be valid");

        assert!(matches!(
            config.validate(),
            Err(ConfigValidationError::DuplicateShortcuts { .. })
        ));
    }

    #[test]
    fn tampered_invalid_shortcut_is_rejected_by_config_validation() {
        let mut config = Config::default();
        config.region_shortcut = Shortcut("not-a-real-key".to_owned());

        assert!(matches!(
            config.validate(),
            Err(ConfigValidationError::InvalidShortcut { .. })
        ));
    }

    #[test]
    fn jpeg_quality_bounds_are_enforced() {
        let mut config = Config::default();
        config.jpeg_quality = 0;
        assert_eq!(
            config.validate(),
            Err(ConfigValidationError::InvalidJpegQuality(0))
        );

        config.jpeg_quality = 100;
        assert!(config.validate().is_ok());
    }

    #[test]
    fn editor_width_index_is_validated() {
        let mut config = Config::default();
        config.editor_stroke_width = 4;
        assert_eq!(
            config.validate(),
            Err(ConfigValidationError::InvalidEditorStrokeWidth(4))
        );
    }

    #[test]
    fn empty_autosave_directory_is_rejected() {
        let mut config = Config::default();
        config.autosave_directory = PathBuf::new();

        assert_eq!(
            config.validate(),
            Err(ConfigValidationError::EmptyAutosaveDirectory)
        );
    }

    #[test]
    fn regular_file_cannot_be_used_as_autosave_directory() {
        let directory = TestDirectory::new();
        let file = directory.path().join("not-a-directory");
        fs::write(&file, b"test").expect("test file should be written");
        let mut config = Config::default();
        config.autosave_directory = file.clone();

        assert_eq!(
            config.validate(),
            Err(ConfigValidationError::AutosaveDirectoryIsFile(file))
        );
    }

    #[test]
    fn toml_round_trip_preserves_all_settings() {
        let config = Config {
            config_version: super::CURRENT_CONFIG_VERSION,
            fullscreen_shortcut: Shortcut::new("alt+F8").expect("valid shortcut"),
            region_shortcut: Shortcut::new("alt+F9").expect("valid shortcut"),
            autosave_directory: PathBuf::from(r"C:\Screenshots"),
            image_format: ScreenshotFormat::Jpeg,
            jpeg_quality: 73,
            png_compression: PngCompression::Best,
            monitor_scope: MonitorScope::VirtualDesktop,
            include_cursor: false,
            region_auto_copy: true,
            region_autosave: true,
            last_editor_tool: EditorTool::Callout,
            editor_color: RgbColor::new(12, 34, 56),
            editor_stroke_width: 3,
        };

        let serialized = toml::to_string_pretty(&config).expect("config should serialize");
        let decoded: Config = toml::from_str(&serialized).expect("config should deserialize");

        assert_eq!(decoded, config);
    }

    #[test]
    fn missing_config_returns_defaults() {
        let directory = TestDirectory::new();
        let path = directory.path().join("missing.toml");

        assert_eq!(
            Config::load_from(path).expect("missing config should use defaults"),
            Config::default()
        );
    }

    #[test]
    fn unversioned_pre_1_0_config_is_loaded_as_schema_one() {
        let directory = TestDirectory::new();
        let path = directory.path().join("config.toml");
        let serialized = toml::to_string_pretty(&Config::default())
            .expect("default config should serialize")
            .replace("config_version = 1\n", "");
        fs::write(&path, serialized).expect("legacy config should be written");

        let loaded = Config::load_from(path).expect("legacy config should migrate implicitly");
        assert_eq!(loaded.config_version, super::CURRENT_CONFIG_VERSION);
    }

    #[test]
    fn unsupported_future_config_schema_is_rejected() {
        let directory = TestDirectory::new();
        let path = directory.path().join("config.toml");
        let serialized = toml::to_string_pretty(&Config::default())
            .expect("default config should serialize")
            .replace("config_version = 1", "config_version = 2");
        fs::write(&path, serialized).expect("future config should be written");

        assert!(matches!(
            Config::load_from(path),
            Err(ConfigError::Validation(
                ConfigValidationError::UnsupportedConfigVersion {
                    found: 2,
                    supported: 1
                }
            ))
        ));
    }

    #[test]
    fn load_rejects_invalid_shortcut_text() {
        let directory = TestDirectory::new();
        let path = directory.path().join("config.toml");
        let config = Config::default();
        let fullscreen_shortcut = config.fullscreen_shortcut.as_str().to_owned();
        let serialized = toml::to_string_pretty(&config)
            .expect("default config should serialize")
            .replace(&fullscreen_shortcut, "control+definitely-not-a-key");
        fs::write(&path, serialized).expect("test config should be written");

        assert!(matches!(
            Config::load_from(path),
            Err(ConfigError::Deserialize { .. })
        ));
    }

    #[test]
    fn save_creates_parent_and_can_atomically_replace_existing_file() {
        let directory = TestDirectory::new();
        let path = directory.path().join("nested").join("config.toml");
        let mut config = Config::default();
        config.save_to(&path).expect("initial save should succeed");

        config.jpeg_quality = 42;
        config.monitor_scope = MonitorScope::VirtualDesktop;
        config
            .save_to(&path)
            .expect("replacement save should succeed");

        let loaded = Config::load_from(&path).expect("saved config should load");
        assert_eq!(loaded, config);

        let temporary_files = fs::read_dir(path.parent().expect("path has a parent"))
            .expect("config directory should be readable")
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
            .count();
        assert_eq!(temporary_files, 0);
    }
}
