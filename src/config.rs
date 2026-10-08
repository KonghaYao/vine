use std::{
    env,
    path::{Path, PathBuf},
};

use garde::Validate;
use rustc_hash::FxHashMap;
use serde::Deserialize;
use smart_default::SmartDefault;
use umbra::optional;

use crate::{
    color::{ColorTheme, OptionalColorTheme},
    graph::GraphImageWidthMode,
    keybind::KeyBind,
    search::SearchTarget,
    CommitOrderType, GraphStyle, GraphWidthType, ImageProtocolType, InitialSelection, Result,
};

const XDG_CONFIG_HOME_ENV_NAME: &str = "XDG_CONFIG_HOME";
const DEFAULT_CONFIG_DIR: &str = ".config";
const APP_DIR_NAME: &str = "vine";
const CONFIG_FILE_NAME: &str = "config.toml";
const CONFIG_FILE_ENV_NAME: &str = "VINE_CONFIG_FILE";

pub fn load() -> Result<(
    CoreConfig,
    UiConfig,
    GraphConfig,
    ActionsConfig,
    ColorTheme,
    Option<KeyBind>,
)> {
    let config = match config_file_path_from_env() {
        Some(user_path) => {
            if !user_path.exists() {
                let msg = format!(
                    "Config file specified by ${CONFIG_FILE_ENV_NAME} environment variable not found: {}",
                    user_path.display()
                );
                return Err(msg.into());
            }
            read_config_from_path(&user_path)
        }
        None => {
            if let Some(default_path) = config_file_path() {
                if default_path.exists() {
                    read_config_from_path(&default_path)
                } else {
                    Ok(Config::default())
                }
            } else {
                Ok(Config::default())
            }
        }
    }?;

    config.validate()?;

    Ok((
        config.core,
        config.ui,
        config.graph,
        config.actions,
        config.color,
        config.keybind,
    ))
}

fn config_file_path_from_env() -> Option<PathBuf> {
    env::var(CONFIG_FILE_ENV_NAME).ok().map(PathBuf::from)
}

fn config_file_path() -> Option<PathBuf> {
    env::var(XDG_CONFIG_HOME_ENV_NAME)
        .ok()
        .map(PathBuf::from)
        .or_else(|| env::home_dir().map(|home| home.join(DEFAULT_CONFIG_DIR)))
        .map(|config_dir| config_dir.join(APP_DIR_NAME).join(CONFIG_FILE_NAME))
}

fn read_config_from_path(path: &Path) -> Result<Config> {
    let content = std::fs::read_to_string(path)?;
    let config: OptionalConfig = toml::from_str(&content)?;
    Ok(config.into())
}

/// Writes the visibility of the optional columns of the commit list to the config file.
///
/// The file is the one that is read at startup (see [`load`]), including the one that is specified
/// by the `$VINE_CONFIG_FILE` environment variable, and it is created when it does not exist yet.
/// Only the keys of the commit list are changed: every other value of the file is kept as it is,
/// including the sections and the keys that the application does not know about. The file is
/// rewritten as a whole, so the comments and the layout of the original file are not kept.
pub fn save_commit_list_config(commit_list: UiCommitListConfig) -> Result<()> {
    match config_file_path_from_env().or_else(config_file_path) {
        Some(path) => write_commit_list_config(&path, commit_list),
        None => Err("The config file is not found, so the setting is not saved".into()),
    }
}

fn write_commit_list_config(path: &Path, commit_list: UiCommitListConfig) -> Result<()> {
    let mut config = if path.exists() {
        let content = std::fs::read_to_string(path)?;
        toml::from_str::<toml::Table>(&content)?
    } else {
        toml::Table::new()
    };
    let table = section(&mut config, "ui", path)?;
    let table = section(table, "commit_list", path)?;
    table.insert("show_date".into(), commit_list.show_date.into());
    table.insert("show_hash".into(), commit_list.show_hash.into());
    table.insert("show_author".into(), commit_list.show_author.into());

    let content = toml::to_string_pretty(&config)?;
    write_atomically(path, &content)
}

/// The table of the given key of the config, created when it does not exist yet.
fn section<'a>(table: &'a mut toml::Table, key: &str, path: &Path) -> Result<&'a mut toml::Table> {
    table
        .entry(key)
        .or_insert_with(|| toml::Value::Table(toml::Table::new()))
        .as_table_mut()
        .ok_or_else(|| format!("The `{key}` section of {} is not a table", path.display()).into())
}

/// Writes the content to the path through a temporary file next to it, so that an interrupted
/// write never leaves a truncated config file behind.
fn write_atomically(path: &Path, content: &str) -> Result<()> {
    let dir = path.parent().filter(|dir| !dir.as_os_str().is_empty());
    if let Some(dir) = dir {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, content)?;
    if let Err(err) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(err.into());
    }
    Ok(())
}

#[optional(derives = [Deserialize])]
#[derive(Debug, Default, Clone, PartialEq, Eq, Validate)]
struct Config {
    #[garde(dive)]
    #[nested]
    core: CoreConfig,
    #[garde(dive)]
    #[nested]
    ui: UiConfig,
    #[garde(dive)]
    #[nested]
    graph: GraphConfig,
    #[garde(dive)]
    #[nested]
    actions: ActionsConfig,
    #[garde(skip)]
    #[nested]
    color: ColorTheme,
    // The user customed keybinds, please ref `assets/default-keybind.toml`
    #[garde(skip)]
    keybind: Option<KeyBind>,
}

#[optional(derives = [Deserialize])]
#[derive(Debug, Default, Clone, PartialEq, Eq, Validate)]
pub struct CoreConfig {
    #[garde(skip)]
    #[nested]
    pub option: CoreOptionConfig,
    #[garde(skip)]
    #[nested]
    pub git: CoreGitConfig,
    #[garde(skip)]
    #[nested]
    pub search: CoreSearchConfig,
    #[garde(dive)]
    #[nested]
    pub user_command: CoreUserCommandConfig,
    #[garde(dive)]
    #[nested]
    pub external: CoreExternalConfig,
}

#[optional(derives = [Deserialize])]
#[derive(Debug, Clone, PartialEq, Eq, SmartDefault)]
pub struct CoreOptionConfig {
    pub protocol: Option<ImageProtocolType>,
    pub order: Option<CommitOrderType>,
    pub graph_width: Option<GraphWidthType>,
    pub graph_style: Option<GraphStyle>,
    pub initial_selection: Option<InitialSelection>,
}

#[optional(derives = [Deserialize])]
#[derive(Debug, Clone, PartialEq, Eq, SmartDefault)]
pub struct CoreGitConfig {
    #[default = false]
    pub mailmap: bool,
}

#[optional(derives = [Deserialize])]
#[derive(Debug, Clone, PartialEq, Eq, SmartDefault)]
pub struct CoreSearchConfig {
    #[default(SearchTarget::All)]
    pub target: SearchTarget,
    #[default = false]
    pub ignore_case: bool,
    #[default = false]
    pub fuzzy: bool,
}

#[optional]
#[derive(Debug, Clone, PartialEq, Eq, SmartDefault, Validate)]
pub struct CoreUserCommandConfig {
    #[garde(dive)]
    #[default(FxHashMap::from_iter([("1".into(), UserCommand {
        name: "git diff".into(),
        r#type: UserCommandType::Inline,
        commands: vec![
            "git".into(),
            "--no-pager".into(),
            "diff".into(),
            "--color=always".into(),
            "{{first_parent_hash}}".into(),
            "{{target_hash}}".into(),
        ],
        refresh: false,
    })]))]
    pub commands: FxHashMap<String, UserCommand>,
    #[garde(range(min = 0))]
    #[default = 4]
    pub tab_width: u16,
}

impl<'de> Deserialize<'de> for OptionalCoreUserCommandConfig {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::{Error, MapAccess, Visitor};
        use std::fmt;

        struct OptionalCoreUserCommandConfigVisitor;

        impl<'de> Visitor<'de> for OptionalCoreUserCommandConfigVisitor {
            type Value = OptionalCoreUserCommandConfig;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a user command configuration")
            }

            fn visit_map<V>(self, mut map: V) -> std::result::Result<Self::Value, V::Error>
            where
                V: MapAccess<'de>,
            {
                let mut commands = FxHashMap::default();
                let mut tab_width = None;

                while let Some(key) = map.next_key::<String>()? {
                    if let Some(suffix) = key.strip_prefix("commands_") {
                        let command_key = suffix.to_string();
                        if command_key.is_empty() {
                            return Err(V::Error::custom(
                                "command key cannot be empty, like `commands_`",
                            ));
                        }
                        let command_value: UserCommand = map.next_value()?;
                        commands.insert(command_key, command_value);
                    } else if key == "tab_width" {
                        tab_width = Some(map.next_value()?);
                    } else if key == "commands" {
                        return Err(V::Error::custom(
                            "invalid key `commands`, use `commands_n` format instead",
                        ));
                    } else {
                        let _: serde::de::IgnoredAny = map.next_value()?;
                    }
                }

                let commands = if commands.is_empty() {
                    None
                } else {
                    Some(commands)
                };

                Ok(OptionalCoreUserCommandConfig {
                    commands,
                    tab_width,
                })
            }
        }

        deserializer.deserialize_map(OptionalCoreUserCommandConfigVisitor)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Validate)]
pub struct UserCommand {
    #[garde(length(min = 1))]
    pub name: String,
    #[serde(default)]
    #[garde(skip)]
    pub r#type: UserCommandType,
    #[garde(length(min = 1), inner(length(min = 1)))]
    pub commands: Vec<String>,
    #[serde(default)]
    #[garde(custom(validate_user_command_refresh(&self.r#type)))]
    pub refresh: bool,
}

fn validate_user_command_refresh(
    command_type: &UserCommandType,
) -> impl FnOnce(&bool, &()) -> garde::Result + '_ {
    move |refresh, _| {
        if matches!(command_type, UserCommandType::Inline) && *refresh {
            return Err(garde::Error::new(
                "refresh cannot be true for inline command",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UserCommandType {
    #[default]
    Inline,
    Silent,
    Suspend,
}

#[optional(derives = [Deserialize])]
#[derive(Debug, Default, Clone, PartialEq, Eq, Validate)]
pub struct UiConfig {
    #[garde(skip)]
    #[nested]
    pub common: UiCommonConfig,
    #[garde(dive)]
    #[nested]
    pub list: UiListConfig,
    #[garde(skip)]
    #[nested]
    pub commit_list: UiCommitListConfig,
    #[garde(dive)]
    #[nested]
    pub detail: UiDetailConfig,
    #[garde(dive)]
    #[nested]
    pub user_command: UiUserCommandConfig,
    #[garde(dive)]
    #[nested]
    pub refs: UiRefsConfig,
}

#[optional(derives = [Deserialize])]
#[derive(Debug, Clone, PartialEq, Eq, SmartDefault)]
pub struct UiCommonConfig {
    #[default(CursorType::Native)]
    pub cursor_type: CursorType,
    #[default = true]
    pub mouse: bool,
    /// Whether to refresh the commit graph automatically when the repository is changed by
    /// another process.
    #[default = true]
    pub auto_refresh: bool,
}

// Config values are lowercase; PascalCase aliases are kept for backward compatibility.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CursorType {
    #[serde(alias = "Native")]
    Native,
    #[serde(alias = "Virtual")]
    Virtual(String),
}

// Config values are lowercase; PascalCase aliases are kept for backward compatibility.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Default, Validate)]
#[serde(rename_all = "lowercase")]
pub enum ClipboardConfig {
    #[default]
    #[serde(alias = "Auto")]
    Auto,
    #[serde(alias = "Custom")]
    Custom {
        #[garde(length(min = 1), inner(length(min = 1)))]
        commands: Vec<String>,
    },
}

#[optional(derives = [Deserialize])]
#[derive(Debug, Clone, PartialEq, Eq, SmartDefault, Validate)]
pub struct CoreExternalConfig {
    #[garde(dive)]
    #[default(ClipboardConfig::Auto)]
    pub clipboard: ClipboardConfig,
}

#[optional(derives = [Deserialize])]
#[derive(Debug, Clone, PartialEq, Eq, SmartDefault, Validate)]
pub struct UiListConfig {
    #[garde(range(min = 0))]
    #[default = 0]
    pub scrolloff: u16,
    #[garde(length(min = 1))]
    #[default(vec![
        UserListColumnType::Graph,
        UserListColumnType::Marker,
        UserListColumnType::Subject,
        UserListColumnType::Name,
        UserListColumnType::Hash,
        UserListColumnType::Date,
    ])]
    pub columns: Vec<UserListColumnType>,
    #[garde(range(min = 1))]
    #[default = 20]
    pub subject_min_width: u16,
    #[garde(length(min = 1))]
    #[default = "%Y-%m-%d"]
    pub date_format: String,
    #[garde(range(min = 0))]
    #[default = 10]
    pub date_width: u16,
    #[garde(skip)]
    #[default = true]
    pub date_local: bool,
    #[garde(range(min = 0))]
    #[default = 12]
    pub name_width: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UserListColumnType {
    Graph,
    Marker,
    Subject,
    Name,
    Hash,
    Date,
}

/// The visibility of the optional columns of the commit list.
///
/// The three columns are toggled from the action menu of a commit, and the values are written back
/// to the config file, so that the choice is kept for the next session.
#[optional(derives = [Deserialize])]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UiCommitListConfig {
    /// Whether the author date of a commit is shown in the commit list.
    pub show_date: bool,
    /// Whether the short hash of a commit is shown in the commit list.
    pub show_hash: bool,
    /// Whether the name of the author of a commit is shown in the commit list.
    pub show_author: bool,
}

impl Default for UiCommitListConfig {
    fn default() -> Self {
        // The three optional columns are shown by default. These are the values to change to flip
        // the default of all of them at once.
        Self {
            show_date: true,
            show_hash: true,
            show_author: true,
        }
    }
}

impl UiCommitListConfig {
    /// Whether the given column is shown.
    pub fn shows(self, column: CommitListColumn) -> bool {
        match column {
            CommitListColumn::Date => self.show_date,
            CommitListColumn::Hash => self.show_hash,
            CommitListColumn::Author => self.show_author,
        }
    }

    /// Shows or hides the given column.
    pub fn set(&mut self, column: CommitListColumn, show: bool) {
        match column {
            CommitListColumn::Date => self.show_date = show,
            CommitListColumn::Hash => self.show_hash = show,
            CommitListColumn::Author => self.show_author = show,
        }
    }

    /// Hides the given column when it is shown, and shows it when it is hidden.
    pub fn toggle(&mut self, column: CommitListColumn) {
        self.set(column, !self.shows(column));
    }

    /// Whether the given column of the commit list is hidden by these settings.
    pub fn hides(self, column: &UserListColumnType) -> bool {
        match CommitListColumn::of_list_column(column) {
            Some(column) => !self.shows(column),
            None => false,
        }
    }
}

/// A column of the commit list that can be shown or hidden from the action menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommitListColumn {
    /// The author date of a commit.
    Date,
    /// The short hash of a commit.
    Hash,
    /// The name of the author of a commit.
    Author,
}

impl CommitListColumn {
    /// The label of the action menu entry that toggles the column.
    pub fn label(self) -> &'static str {
        match self {
            CommitListColumn::Date => "Date",
            CommitListColumn::Hash => "Hash",
            CommitListColumn::Author => "Author",
        }
    }

    /// The column of `ui.list.columns` that the switch shows or hides.
    pub fn list_column(self) -> UserListColumnType {
        match self {
            CommitListColumn::Date => UserListColumnType::Date,
            CommitListColumn::Hash => UserListColumnType::Hash,
            CommitListColumn::Author => UserListColumnType::Name,
        }
    }

    /// The switch that shows or hides the given column of `ui.list.columns`, or `None` when the
    /// column is not one of the optional ones.
    pub fn of_list_column(column: &UserListColumnType) -> Option<Self> {
        match column {
            UserListColumnType::Date => Some(CommitListColumn::Date),
            UserListColumnType::Hash => Some(CommitListColumn::Hash),
            UserListColumnType::Name => Some(CommitListColumn::Author),
            UserListColumnType::Graph
            | UserListColumnType::Marker
            | UserListColumnType::Subject => None,
        }
    }

    /// The switches that the commit list actually uses, in the order of the menu.
    ///
    /// A column that is not in `ui.list.columns` is never rendered, so it has no switch: a switch
    /// is always in the same state as the column it controls.
    pub fn used_by(columns: &[UserListColumnType]) -> Vec<Self> {
        [
            CommitListColumn::Date,
            CommitListColumn::Hash,
            CommitListColumn::Author,
        ]
        .into_iter()
        .filter(|column| columns.contains(&column.list_column()))
        .collect()
    }
}

#[optional(derives = [Deserialize])]
#[derive(Debug, Clone, PartialEq, Eq, SmartDefault, Validate)]
pub struct UiDetailConfig {
    #[garde(range(min = 1))]
    #[default = 20]
    pub height: u16,
    #[garde(length(min = 1))]
    #[default = "%Y-%m-%d %H:%M:%S %z"]
    pub date_format: String,
    #[garde(skip)]
    #[default = true]
    pub date_local: bool,
}

#[optional(derives = [Deserialize])]
#[derive(Debug, Clone, PartialEq, Eq, SmartDefault, Validate)]
pub struct UiUserCommandConfig {
    #[garde(range(min = 1))]
    #[default = 20]
    pub height: u16,
}

#[optional(derives = [Deserialize])]
#[derive(Debug, Clone, PartialEq, Eq, SmartDefault, Validate)]
pub struct UiRefsConfig {
    #[garde(range(min = 1))]
    #[default = 26]
    pub width: u16,
}

/// Settings for the git actions executed from the action menu.
#[optional(derives = [Deserialize])]
#[derive(Debug, Clone, PartialEq, Eq, SmartDefault, Validate)]
pub struct ActionsConfig {
    #[garde(skip)]
    #[default = true]
    pub enable: bool,
    #[garde(skip)]
    #[default = true]
    pub confirm_destructive: bool,
}

#[optional(derives = [Deserialize])]
#[derive(Debug, Clone, PartialEq, Eq, SmartDefault, Validate)]
pub struct GraphConfig {
    #[garde(skip)]
    #[default(GraphImageWidthMode::Compact)]
    pub row_image_width: GraphImageWidthMode,
    #[garde(dive)]
    #[nested]
    pub color: GraphColorConfig,
}

#[optional(derives = [Deserialize])]
#[derive(Debug, Clone, PartialEq, Eq, SmartDefault, Validate)]
pub struct GraphColorConfig {
    #[garde(length(min = 1), inner(pattern(r"^#([0-9a-fA-F]{6}|[0-9a-fA-F]{8})$")))]
    #[default(vec![
        "#E06C76".into(),
        "#98C379".into(),
        "#E5C07B".into(),
        "#61AFEF".into(),
        "#C678DD".into(),
        "#56B6C2".into(),
    ])]
    pub branches: Vec<String>,
    #[garde(pattern(r"^#([0-9a-fA-F]{6}|[0-9a-fA-F]{8})$"))]
    #[default = "#00000000"]
    pub edge: String,
    #[garde(pattern(r"^#([0-9a-fA-F]{6}|[0-9a-fA-F]{8})$"))]
    #[default = "#00000000"]
    pub background: String,
}

#[cfg(test)]
mod tests {
    use ratatui::style::Color as RatatuiColor;

    use super::*;

    #[test]
    fn test_config_default() {
        let actual = Config::default();
        let expected = Config {
            core: CoreConfig {
                option: CoreOptionConfig {
                    protocol: None,
                    order: None,
                    graph_width: None,
                    graph_style: None,
                    initial_selection: None,
                },
                git: CoreGitConfig { mailmap: false },
                search: CoreSearchConfig {
                    target: SearchTarget::All,
                    ignore_case: false,
                    fuzzy: false,
                },
                user_command: CoreUserCommandConfig {
                    commands: FxHashMap::from_iter([(
                        "1".into(),
                        UserCommand {
                            name: "git diff".into(),
                            r#type: UserCommandType::Inline,
                            commands: vec![
                                "git".into(),
                                "--no-pager".into(),
                                "diff".into(),
                                "--color=always".into(),
                                "{{first_parent_hash}}".into(),
                                "{{target_hash}}".into(),
                            ],
                            refresh: false,
                        },
                    )]),
                    tab_width: 4,
                },
                external: CoreExternalConfig {
                    clipboard: ClipboardConfig::Auto,
                },
            },
            ui: UiConfig {
                common: UiCommonConfig {
                    cursor_type: CursorType::Native,
                    mouse: true,
                    auto_refresh: true,
                },
                list: UiListConfig {
                    scrolloff: 0,
                    columns: vec![
                        UserListColumnType::Graph,
                        UserListColumnType::Marker,
                        UserListColumnType::Subject,
                        UserListColumnType::Name,
                        UserListColumnType::Hash,
                        UserListColumnType::Date,
                    ],
                    subject_min_width: 20,
                    date_format: "%Y-%m-%d".into(),
                    date_width: 10,
                    date_local: true,
                    name_width: 12,
                },
                commit_list: UiCommitListConfig {
                    show_date: true,
                    show_hash: true,
                    show_author: true,
                },
                detail: UiDetailConfig {
                    height: 20,
                    date_format: "%Y-%m-%d %H:%M:%S %z".into(),
                    date_local: true,
                },
                user_command: UiUserCommandConfig { height: 20 },
                refs: UiRefsConfig { width: 26 },
            },
            graph: GraphConfig {
                row_image_width: GraphImageWidthMode::Compact,
                color: GraphColorConfig {
                    branches: vec![
                        "#E06C76".into(),
                        "#98C379".into(),
                        "#E5C07B".into(),
                        "#61AFEF".into(),
                        "#C678DD".into(),
                        "#56B6C2".into(),
                    ],
                    edge: "#00000000".into(),
                    background: "#00000000".into(),
                },
            },
            actions: ActionsConfig {
                enable: true,
                confirm_destructive: true,
            },
            color: ColorTheme::default(),
            keybind: None,
        };
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_config_complete_toml() {
        let toml = r##"
            [core.option]
            protocol = "kitty-unicode"
            order = "topo"
            graph_width = "single"
            graph_style = "angular"
            initial_selection = "head"
            [core.git]
            mailmap = true
            [core.search]
            target = "author"
            ignore_case = true
            fuzzy = true
            [core.user_command]
            commands_1 = { name = "git diff no color", commands = ["git", "diff", "{{first_parent_hash}}", "{{target_hash}}"] }
            commands_2 = { name = "echo hello", type = "silent", commands = ["echo", "hello"], refresh = true }
            commands_10 = { name = "echo world", type = "inline", commands = ["echo", "world"], refresh = false }
            commands_3 = { name = "open vim", type = "suspend", commands = ["vim"] }
            tab_width = 2
            [ui.common]
            cursor_type = { virtual = "|" }
            mouse = false
            auto_refresh = false
            [ui.list]
            scrolloff = 3
            columns = ["date", "subject", "hash", "graph"]
            subject_min_width = 40
            date_format = "%Y/%m/%d"
            date_width = 20
            date_local = false
            name_width = 30
            [ui.detail]
            height = 30
            date_format = "%Y/%m/%d %H:%M:%S"
            date_local = false
            [ui.user_command]
            height = 30
            [ui.refs]
            width = 40
            [actions]
            enable = false
            confirm_destructive = false
            [graph]
            row_image_width = "fixed"
            [graph.color]
            branches = ["#ff0000", "#00ff00", "#0000ff"]
            edge = "#000000"
            background = "#ffffff"
            [color]
            status_bar_fg = "black"
            status_bar_bg = "red"
        "##;
        let actual: Config = toml::from_str::<OptionalConfig>(toml).unwrap().into();
        let expected = Config {
            core: CoreConfig {
                option: CoreOptionConfig {
                    protocol: Some(ImageProtocolType::KittyUnicode),
                    order: Some(CommitOrderType::Topo),
                    graph_width: Some(GraphWidthType::Single),
                    graph_style: Some(GraphStyle::Angular),
                    initial_selection: Some(InitialSelection::Head),
                },
                git: CoreGitConfig { mailmap: true },
                search: CoreSearchConfig {
                    target: SearchTarget::Author,
                    ignore_case: true,
                    fuzzy: true,
                },
                user_command: CoreUserCommandConfig {
                    commands: FxHashMap::from_iter([
                        (
                            "1".into(),
                            UserCommand {
                                name: "git diff no color".into(),
                                r#type: UserCommandType::Inline,
                                commands: vec![
                                    "git".into(),
                                    "diff".into(),
                                    "{{first_parent_hash}}".into(),
                                    "{{target_hash}}".into(),
                                ],
                                refresh: false,
                            },
                        ),
                        (
                            "2".into(),
                            UserCommand {
                                name: "echo hello".into(),
                                r#type: UserCommandType::Silent,
                                commands: vec!["echo".into(), "hello".into()],
                                refresh: true,
                            },
                        ),
                        (
                            "3".into(),
                            UserCommand {
                                name: "open vim".into(),
                                r#type: UserCommandType::Suspend,
                                commands: vec!["vim".into()],
                                refresh: false,
                            },
                        ),
                        (
                            "10".into(),
                            UserCommand {
                                name: "echo world".into(),
                                r#type: UserCommandType::Inline,
                                commands: vec!["echo".into(), "world".into()],
                                refresh: false,
                            },
                        ),
                    ]),
                    tab_width: 2,
                },
                external: CoreExternalConfig {
                    clipboard: ClipboardConfig::Auto,
                },
            },
            ui: UiConfig {
                common: UiCommonConfig {
                    cursor_type: CursorType::Virtual("|".into()),
                    mouse: false,
                    auto_refresh: false,
                },
                list: UiListConfig {
                    scrolloff: 3,
                    columns: vec![
                        UserListColumnType::Date,
                        UserListColumnType::Subject,
                        UserListColumnType::Hash,
                        UserListColumnType::Graph,
                    ],
                    subject_min_width: 40,
                    date_format: "%Y/%m/%d".into(),
                    date_width: 20,
                    date_local: false,
                    name_width: 30,
                },
                commit_list: UiCommitListConfig {
                    show_date: true,
                    show_hash: true,
                    show_author: true,
                },
                detail: UiDetailConfig {
                    height: 30,
                    date_format: "%Y/%m/%d %H:%M:%S".into(),
                    date_local: false,
                },
                user_command: UiUserCommandConfig { height: 30 },
                refs: UiRefsConfig { width: 40 },
            },
            graph: GraphConfig {
                row_image_width: GraphImageWidthMode::Fixed,
                color: GraphColorConfig {
                    branches: vec!["#ff0000".into(), "#00ff00".into(), "#0000ff".into()],
                    edge: "#000000".into(),
                    background: "#ffffff".into(),
                },
            },
            actions: ActionsConfig {
                enable: false,
                confirm_destructive: false,
            },
            color: ColorTheme {
                status_bar_fg: RatatuiColor::Black,
                status_bar_bg: RatatuiColor::Red,
                ..ColorTheme::default()
            },
            keybind: None,
        };
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_config_color_status_bar() {
        // the status bar is white on blue by default
        let actual = Config::default();
        assert_eq!(actual.color.status_bar_fg, RatatuiColor::White);
        assert_eq!(actual.color.status_bar_bg, RatatuiColor::Blue);

        let toml = r#"
            [color]
            status_bar_fg = "black"
            status_bar_bg = "red"
        "#;
        let actual: Config = toml::from_str::<OptionalConfig>(toml).unwrap().into();
        assert_eq!(actual.color.status_bar_fg, RatatuiColor::Black);
        assert_eq!(actual.color.status_bar_bg, RatatuiColor::Red);

        // the other colors keep their default
        assert_eq!(
            actual.color.status_info_fg,
            ColorTheme::default().status_info_fg
        );
    }

    #[test]
    fn test_config_partial_toml() {
        let toml = r#"
            [ui.common]
            cursor_type = "native"
            [ui.list]
            date_format = "%Y/%m/%d"
        "#;
        let actual: Config = toml::from_str::<OptionalConfig>(toml).unwrap().into();
        let expected = Config {
            core: CoreConfig {
                option: CoreOptionConfig {
                    protocol: None,
                    order: None,
                    graph_width: None,
                    graph_style: None,
                    initial_selection: None,
                },
                git: CoreGitConfig { mailmap: false },
                search: CoreSearchConfig {
                    target: SearchTarget::All,
                    ignore_case: false,
                    fuzzy: false,
                },
                user_command: CoreUserCommandConfig {
                    commands: FxHashMap::from_iter([(
                        "1".into(),
                        UserCommand {
                            name: "git diff".into(),
                            r#type: UserCommandType::Inline,
                            commands: vec![
                                "git".into(),
                                "--no-pager".into(),
                                "diff".into(),
                                "--color=always".into(),
                                "{{first_parent_hash}}".into(),
                                "{{target_hash}}".into(),
                            ],
                            refresh: false,
                        },
                    )]),
                    tab_width: 4,
                },
                external: CoreExternalConfig {
                    clipboard: ClipboardConfig::Auto,
                },
            },
            ui: UiConfig {
                common: UiCommonConfig {
                    cursor_type: CursorType::Native,
                    mouse: true,
                    auto_refresh: true,
                },
                list: UiListConfig {
                    scrolloff: 0,
                    columns: vec![
                        UserListColumnType::Graph,
                        UserListColumnType::Marker,
                        UserListColumnType::Subject,
                        UserListColumnType::Name,
                        UserListColumnType::Hash,
                        UserListColumnType::Date,
                    ],
                    subject_min_width: 20,
                    date_format: "%Y/%m/%d".into(),
                    date_width: 10,
                    date_local: true,
                    name_width: 12,
                },
                commit_list: UiCommitListConfig {
                    show_date: true,
                    show_hash: true,
                    show_author: true,
                },
                detail: UiDetailConfig {
                    height: 20,
                    date_format: "%Y-%m-%d %H:%M:%S %z".into(),
                    date_local: true,
                },
                user_command: UiUserCommandConfig { height: 20 },
                refs: UiRefsConfig { width: 26 },
            },
            graph: GraphConfig {
                row_image_width: GraphImageWidthMode::Compact,
                color: GraphColorConfig {
                    branches: vec![
                        "#E06C76".into(),
                        "#98C379".into(),
                        "#E5C07B".into(),
                        "#61AFEF".into(),
                        "#C678DD".into(),
                        "#56B6C2".into(),
                    ],
                    edge: "#00000000".into(),
                    background: "#00000000".into(),
                },
            },
            actions: ActionsConfig {
                enable: true,
                confirm_destructive: true,
            },
            color: ColorTheme::default(),
            keybind: None,
        };
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_config_ui_common_mouse_defaults_to_true() {
        let config: Config = toml::from_str::<OptionalConfig>("").unwrap().into();
        assert!(config.ui.common.mouse);
    }

    #[test]
    fn test_config_ui_common_mouse_can_be_disabled() {
        let toml = r#"
            [ui.common]
            mouse = false
        "#;
        let config: Config = toml::from_str::<OptionalConfig>(toml).unwrap().into();
        assert!(!config.ui.common.mouse);
        assert_eq!(config.ui.common.cursor_type, CursorType::Native);
    }

    #[test]
    fn test_config_ui_common_auto_refresh_defaults_to_true() {
        let config: Config = toml::from_str::<OptionalConfig>("").unwrap().into();
        assert!(config.ui.common.auto_refresh);
    }

    #[test]
    fn test_config_ui_common_auto_refresh_can_be_disabled() {
        let toml = r#"
            [ui.common]
            auto_refresh = false
        "#;
        let config: Config = toml::from_str::<OptionalConfig>(toml).unwrap().into();
        assert!(!config.ui.common.auto_refresh);
        assert!(config.ui.common.mouse);
        assert_eq!(config.ui.common.cursor_type, CursorType::Native);
    }

    #[test]
    fn test_config_actions_defaults() {
        let config: Config = toml::from_str::<OptionalConfig>("").unwrap().into();
        assert!(config.actions.enable);
        assert!(config.actions.confirm_destructive);
    }

    #[test]
    fn test_config_actions_can_be_disabled() {
        let toml = r#"
            [actions]
            enable = false
            confirm_destructive = false
        "#;
        let config: Config = toml::from_str::<OptionalConfig>(toml).unwrap().into();
        assert!(!config.actions.enable);
        assert!(!config.actions.confirm_destructive);
    }

    #[test]
    fn test_config_clipboard_auto() {
        let toml = r#"
            [core.external]
            clipboard = "auto"
        "#;
        let config: Config = toml::from_str::<OptionalConfig>(toml).unwrap().into();
        assert_eq!(config.core.external.clipboard, ClipboardConfig::Auto);
    }

    #[test]
    fn test_config_clipboard_custom_single_command() {
        let toml = r#"
            [core.external]
            clipboard = { custom = { commands = ["wl-copy"] } }
        "#;
        let config: Config = toml::from_str::<OptionalConfig>(toml).unwrap().into();
        assert_eq!(
            config.core.external.clipboard,
            ClipboardConfig::Custom {
                commands: vec!["wl-copy".into()]
            }
        );
    }

    #[test]
    fn test_config_clipboard_custom_command_with_args() {
        let toml = r#"
            [core.external]
            clipboard = { Custom = { commands = ["xclip", "-selection", "clipboard"] } }
        "#;
        let config: Config = toml::from_str::<OptionalConfig>(toml).unwrap().into();
        assert_eq!(
            config.core.external.clipboard,
            ClipboardConfig::Custom {
                commands: vec!["xclip".into(), "-selection".into(), "clipboard".into()]
            }
        );
    }

    #[test]
    fn test_config_commit_list_columns_are_shown_by_default() {
        assert_eq!(
            UiCommitListConfig::default(),
            UiCommitListConfig {
                show_date: true,
                show_hash: true,
                show_author: true,
            }
        );
        let config: Config = toml::from_str::<OptionalConfig>("").unwrap().into();
        assert_eq!(config.ui.commit_list, UiCommitListConfig::default());
    }

    #[test]
    fn test_config_commit_list_columns_can_be_hidden() {
        let toml = r#"
            [ui.commit_list]
            show_date = false
            show_hash = true
            show_author = false
        "#;
        let config: Config = toml::from_str::<OptionalConfig>(toml).unwrap().into();
        assert_eq!(
            config.ui.commit_list,
            UiCommitListConfig {
                show_date: false,
                show_hash: true,
                show_author: false,
            }
        );
    }

    #[test]
    fn test_config_commit_list_columns_default_one_by_one() {
        let toml = r#"
            [ui.commit_list]
            show_date = false
        "#;
        let config: Config = toml::from_str::<OptionalConfig>(toml).unwrap().into();
        assert!(!config.ui.commit_list.show_date);
        assert!(
            config.ui.commit_list.show_hash && config.ui.commit_list.show_author,
            "the keys that are not in the config file keep their default"
        );
    }

    #[test]
    fn test_commit_list_columns_toggle_and_hide_the_columns_of_the_list() {
        let mut commit_list = UiCommitListConfig::default();
        for column in [
            CommitListColumn::Date,
            CommitListColumn::Hash,
            CommitListColumn::Author,
        ] {
            assert!(commit_list.shows(column));
        }

        commit_list.toggle(CommitListColumn::Hash);
        assert!(!commit_list.shows(CommitListColumn::Hash));
        assert!(
            commit_list.shows(CommitListColumn::Date)
                && commit_list.shows(CommitListColumn::Author),
            "only the toggled column is changed"
        );
        commit_list.toggle(CommitListColumn::Hash);
        assert!(commit_list.shows(CommitListColumn::Hash));

        let all_columns = [
            UserListColumnType::Graph,
            UserListColumnType::Marker,
            UserListColumnType::Subject,
            UserListColumnType::Name,
            UserListColumnType::Hash,
            UserListColumnType::Date,
        ];
        assert_eq!(
            all_columns
                .iter()
                .filter(|column| commit_list.hides(column))
                .count(),
            0
        );

        let mut commit_list = UiCommitListConfig::default();
        for column in [
            CommitListColumn::Date,
            CommitListColumn::Hash,
            CommitListColumn::Author,
        ] {
            commit_list.toggle(column);
        }
        assert_eq!(
            &all_columns
                .iter()
                .filter(|column| commit_list.hides(column))
                .collect::<Vec<_>>(),
            &[
                &UserListColumnType::Name,
                &UserListColumnType::Hash,
                &UserListColumnType::Date
            ],
            "the graph, the marker and the subject are never hidden"
        );
    }

    #[test]
    fn test_the_column_switches_follow_the_columns_of_the_commit_list() {
        for column in [
            CommitListColumn::Date,
            CommitListColumn::Hash,
            CommitListColumn::Author,
        ] {
            assert_eq!(
                CommitListColumn::of_list_column(&column.list_column()),
                Some(column),
                "a switch and the column it controls are mapped to each other"
            );
        }
        for column in [
            UserListColumnType::Graph,
            UserListColumnType::Marker,
            UserListColumnType::Subject,
        ] {
            assert_eq!(
                CommitListColumn::of_list_column(&column),
                None,
                "{column:?} has no switch"
            );
        }

        assert_eq!(
            CommitListColumn::used_by(&UiListConfig::default().columns),
            vec![
                CommitListColumn::Date,
                CommitListColumn::Hash,
                CommitListColumn::Author
            ],
            "the default configuration uses the three optional columns"
        );
        assert_eq!(
            CommitListColumn::used_by(&[
                UserListColumnType::Subject,
                UserListColumnType::Name,
                UserListColumnType::Date,
            ]),
            vec![CommitListColumn::Date, CommitListColumn::Author],
            "the columns that are not used have no switch"
        );
        assert!(
            CommitListColumn::used_by(&[UserListColumnType::Graph, UserListColumnType::Subject])
                .is_empty(),
            "a list without an optional column has no switch"
        );

        // The switches hide a column of the list, and they never show a column that is not used
        let hidden = UiCommitListConfig {
            show_date: false,
            show_hash: true,
            show_author: false,
        };
        assert!(hidden.hides(&UserListColumnType::Date));
        assert!(!hidden.hides(&UserListColumnType::Hash));
        assert!(hidden.hides(&UserListColumnType::Name));
        assert!(
            !hidden.hides(&UserListColumnType::Subject)
                && !hidden.hides(&UserListColumnType::Graph)
                && !hidden.hides(&UserListColumnType::Marker),
            "the other columns are never hidden by the switches"
        );
    }

    #[test]
    fn test_saving_the_commit_list_columns_keeps_the_other_values() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            r#"
            [core.git]
            mailmap = true
            untouched = "kept"

            [core.user_command]
            commands_1 = { name = "git diff", commands = ["git", "diff"] }

            [ui.list]
            date_format = "%Y/%m/%d"
            columns = ["graph", "subject", "date"]

            [ui.commit_list]
            show_date = true
            show_hash = true
            show_author = true
        "#,
        )
        .unwrap();

        let saved = UiCommitListConfig {
            show_date: false,
            show_hash: true,
            show_author: false,
        };
        write_commit_list_config(&path, saved).unwrap();
        assert_eq!(
            std::fs::read_to_string(&path)
                .unwrap()
                .parse::<toml::Table>()
                .unwrap(),
            toml::from_str::<toml::Table>(
                r#"
                [core.git]
                mailmap = true
                untouched = "kept"

                [core.user_command]
                commands_1 = { name = "git diff", commands = ["git", "diff"] }

                [ui.list]
                date_format = "%Y/%m/%d"
                columns = ["graph", "subject", "date"]

                [ui.commit_list]
                show_date = false
                show_hash = true
                show_author = false
            "#
            )
            .unwrap(),
        );

        // The whole config is still read back, with the saved columns and the other values
        let config = read_config_from_path(&path).unwrap();
        assert_eq!(config.ui.commit_list, saved);
        assert!(config.core.git.mailmap);
        assert_eq!(config.ui.list.date_format, "%Y/%m/%d");
        assert_eq!(
            config.ui.list.columns,
            vec![
                UserListColumnType::Graph,
                UserListColumnType::Subject,
                UserListColumnType::Date
            ]
        );
        assert_eq!(
            config.core.user_command.commands["1"].name, "git diff",
            "the inline user command is still read back"
        );
    }

    #[test]
    fn test_saving_the_commit_list_columns_creates_the_config_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("vine").join("config.toml");
        let saved = UiCommitListConfig {
            show_date: true,
            show_hash: false,
            show_author: true,
        };

        write_commit_list_config(&path, saved).unwrap();
        assert!(path.exists());
        let config = read_config_from_path(&path).unwrap();
        assert_eq!(config.ui.commit_list, saved);
        assert_eq!(
            config.ui.list,
            UiListConfig::default(),
            "the keys that are not saved keep their default"
        );
        assert!(
            !path.with_extension("toml.tmp").exists(),
            "the temporary file used by the atomic write is removed"
        );
    }
}
