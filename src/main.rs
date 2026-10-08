mod app;
mod check;
mod color;
mod config;
mod event;
mod external;
mod git;
mod git_action;
mod graph;
mod keybind;
mod protocol;
mod search;
mod view;
mod watch;
mod widget;

#[cfg(test)]
#[path = "tests/graph.rs"]
mod graph_tests;

#[cfg(test)]
#[path = "tests/mailmap.rs"]
mod mailmap_tests;

#[cfg(test)]
#[path = "tests/git.rs"]
mod test_git;

#[cfg(test)]
#[path = "tests/watch.rs"]
mod watch_tests;

use std::{cell::Cell, env, path::Path, rc::Rc};

use app::{App, Ret};
use clap::{Parser, ValueEnum};
use graph::GraphImageManager;
use serde::Deserialize;

/// Vine - A rich git commit graph in your terminal, like magic 📚
#[derive(Parser)]
#[command(version)]
struct Args {
    /// Maximum number of commits to render
    #[arg(short = 'n', long, value_name = "NUMBER")]
    max_count: Option<usize>,

    /// Image protocol to render graph [default: ascii]
    #[arg(short, long, value_name = "TYPE")]
    protocol: Option<ImageProtocolType>,

    /// Commit ordering algorithm [default: chrono]
    #[arg(short, long, value_name = "TYPE")]
    order: Option<CommitOrderType>,

    /// Commit graph image cell width [default: auto]
    #[arg(short, long, value_name = "TYPE")]
    graph_width: Option<GraphWidthType>,

    /// Commit graph image edge style [default: rounded]
    #[arg(short = 's', long, value_name = "TYPE")]
    graph_style: Option<GraphStyle>,

    /// Initial selection of commit [default: latest]
    #[arg(short, long, value_name = "TYPE")]
    initial_selection: Option<InitialSelection>,

    /// Primary branch to keep on the leftmost column
    #[arg(short = 'b', long, value_name = "BRANCH")]
    primary_branch: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum ImageProtocolType {
    Auto,
    Iterm,
    Kitty,
    KittyUnicode,
    Ascii,
}

/// The environment variable that forces the image protocol, taking precedence over the config file.
const PROTOCOL_ENV_NAME: &str = "VINE_PROTOCOL";

fn parse_protocol_type(value: &str) -> Option<ImageProtocolType> {
    ImageProtocolType::from_str(value.trim(), true).ok()
}

/// The protocol type forced by `$VINE_PROTOCOL`, if any.
///
/// This takes precedence over the `protocol` config option, so that the mode can be forced per
/// environment (e.g. `VINE_PROTOCOL=ascii` in a shell on a remote host) without editing the config.
fn protocol_type_from_env() -> Result<Option<ImageProtocolType>> {
    protocol_type_from_value(env::var(PROTOCOL_ENV_NAME).ok())
}

fn protocol_type_from_value(value: Option<String>) -> Result<Option<ImageProtocolType>> {
    let Some(value) = value else {
        return Ok(None);
    };
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    parse_protocol_type(value).map(Some).ok_or_else(|| {
        let possible_values = ImageProtocolType::value_variants()
            .iter()
            .filter_map(|v| v.to_possible_value())
            .map(|v| v.get_name().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "invalid value for ${PROTOCOL_ENV_NAME}: {value:?} (possible values: {possible_values})"
        )
        .into()
    })
}

/// Resolves the protocol type to use, in order of precedence: the `--protocol` option, the
/// `$VINE_PROTOCOL` environment variable, and the `protocol` config option.
///
/// `None` means that none of them is specified, and the default is used.
fn resolve_protocol_type(
    protocol: Option<ImageProtocolType>,
    env_protocol: Result<Option<ImageProtocolType>>,
    config_protocol: Option<ImageProtocolType>,
) -> Result<Option<ImageProtocolType>> {
    // The environment variable is parsed even when the option takes precedence over it, so that a
    // typo in the environment is reported instead of being silently ignored.
    Ok(protocol.or(env_protocol?).or(config_protocol))
}

impl From<Option<ImageProtocolType>> for protocol::ImageProtocol {
    fn from(protocol: Option<ImageProtocolType>) -> Self {
        match protocol {
            Some(ImageProtocolType::Auto) => protocol::auto_detect(),
            Some(ImageProtocolType::Iterm) => protocol::ImageProtocol::Iterm2,
            Some(ImageProtocolType::Kitty) => protocol::ImageProtocol::Kitty,
            Some(ImageProtocolType::KittyUnicode) => protocol::ImageProtocol::KittyUnicode {
                tmux: protocol::detect_tmux(),
            },
            // The ASCII graph is the default: the image protocols and the terminal detection have
            // to be requested explicitly, so that the graph is always visible.
            Some(ImageProtocolType::Ascii) | None => protocol::ImageProtocol::Ascii,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Deserialize)]
#[serde(rename_all = "lowercase")]
enum CommitOrderType {
    Chrono,
    Topo,
}

impl From<Option<CommitOrderType>> for git::SortCommit {
    fn from(order: Option<CommitOrderType>) -> Self {
        match order {
            Some(CommitOrderType::Chrono) => git::SortCommit::Chronological,
            Some(CommitOrderType::Topo) => git::SortCommit::Topological,
            None => git::SortCommit::Chronological,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Deserialize)]
#[serde(rename_all = "lowercase")]
enum GraphWidthType {
    Auto,
    Double,
    Single,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Deserialize)]
#[serde(rename_all = "lowercase")]
enum GraphStyle {
    Rounded,
    Angular,
}

impl From<Option<GraphStyle>> for graph::GraphStyle {
    fn from(style: Option<GraphStyle>) -> Self {
        match style {
            Some(GraphStyle::Rounded) => graph::GraphStyle::Rounded,
            Some(GraphStyle::Angular) => graph::GraphStyle::Angular,
            None => graph::GraphStyle::Rounded,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Deserialize)]
#[serde(rename_all = "lowercase")]
enum InitialSelection {
    Latest,
    Head,
}

impl From<Option<InitialSelection>> for app::InitialSelection {
    fn from(selection: Option<InitialSelection>) -> Self {
        match selection {
            Some(InitialSelection::Latest) => app::InitialSelection::Latest,
            Some(InitialSelection::Head) => app::InitialSelection::Head,
            None => app::InitialSelection::Latest,
        }
    }
}

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn main() -> Result<()> {
    let args = Args::parse();
    let (core_config, ui_config, graph_config, actions_config, color_theme, keybind_patch) =
        config::load()?;
    let keybind = keybind::KeyBind::new(keybind_patch);

    let max_count = args.max_count;
    let image_protocol = resolve_protocol_type(
        args.protocol,
        protocol_type_from_env(),
        core_config.option.protocol,
    )?
    .into();
    let order = args.order.or(core_config.option.order).into();
    let graph_width = args.graph_width.or(core_config.option.graph_width);
    let graph_style = args.graph_style.or(core_config.option.graph_style).into();
    let graph_image_width_mode = graph_config.row_image_width;
    let initial_selection = args
        .initial_selection
        .or(core_config.option.initial_selection)
        .into();
    let mailmap = core_config.git.mailmap;
    let primary_branch = args.primary_branch.filter(|s| !s.is_empty());

    let graph_color_set = color::GraphColorSet::new(&graph_config.color);

    // The commit list is rendered with this value, which the action menu changes while it runs
    let commit_list_config = ui_config.commit_list;

    let ctx = Rc::new(app::AppContext {
        keybind,
        core_config,
        ui_config,
        actions_config,
        color_theme,
        image_protocol,
        commit_list_config: Cell::new(commit_list_config),
    });

    let ec = event::EventController::init(ctx.ui_config.common.mouse);
    if ctx.ui_config.common.auto_refresh {
        ec.watch_repository(Path::new("."));
    }
    let mut refresh_view_context = None;
    let mut terminal = None;

    let ret = loop {
        let repository = git::Repository::load(Path::new("."), order, max_count, mailmap)?;
        // The changes of the repository seen so far are part of what was just loaded.
        ec.mark_repository_loaded();

        let graph = graph::calc_graph(&repository, primary_branch.as_deref());

        let cell_width_type = check::decide_cell_width_type(&graph, graph_width)?;

        let graph_image_manager = GraphImageManager::new(
            &graph,
            &graph_color_set,
            cell_width_type,
            graph_style,
            graph_image_width_mode,
            image_protocol,
        );

        if terminal.is_none() {
            terminal = Some(ratatui::init());
            if ctx.ui_config.common.mouse {
                // `ratatui::init()` restores the terminal on panic, but it does not disable the
                // mouse capture, so install an additional hook after it.
                event::install_mouse_capture_panic_hook();
                ec.enable_mouse_capture();
            }
        }

        let mut app = App::new(
            &repository,
            graph_image_manager,
            &graph_color_set,
            initial_selection,
            ctx.clone(),
            &ec,
            refresh_view_context,
        );

        match app.run(terminal.as_mut().unwrap()) {
            Ok(Ret::Quit) => {
                break Ok(());
            }
            Ok(Ret::Refresh(request)) => {
                refresh_view_context = Some(request.context);
                continue;
            }
            Err(e) => {
                break Err(e);
            }
        }
    };

    ec.disable_mouse_capture();
    ratatui::restore();
    ret.map_err(Into::into)
}

#[cfg(test)]
mod protocol_env_tests {
    use super::*;

    #[test]
    fn test_protocol_type_from_value_accepts_the_option_values() {
        assert_eq!(
            protocol_type_from_value(Some("ascii".into())).unwrap(),
            Some(ImageProtocolType::Ascii)
        );
        assert_eq!(
            protocol_type_from_value(Some("ASCII".into())).unwrap(),
            Some(ImageProtocolType::Ascii)
        );
        assert_eq!(
            protocol_type_from_value(Some(" kitty-unicode ".into())).unwrap(),
            Some(ImageProtocolType::KittyUnicode)
        );
        assert_eq!(
            protocol_type_from_value(Some("auto".into())).unwrap(),
            Some(ImageProtocolType::Auto)
        );
    }

    #[test]
    fn test_protocol_type_from_value_ignores_unset_and_empty_values() {
        assert_eq!(protocol_type_from_value(None).unwrap(), None);
        assert_eq!(protocol_type_from_value(Some("".into())).unwrap(), None);
        assert_eq!(protocol_type_from_value(Some("  ".into())).unwrap(), None);
    }

    #[test]
    fn test_protocol_type_from_value_reports_an_invalid_value() {
        let err = protocol_type_from_value(Some("image".into())).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("VINE_PROTOCOL"), "{msg}");
        assert!(msg.contains("kitty-unicode"), "{msg}");
    }

    #[test]
    fn test_resolve_protocol_type_prefers_the_option_then_the_env_then_the_config() {
        let env = || Ok(Some(ImageProtocolType::KittyUnicode));
        let config = Some(ImageProtocolType::Kitty);

        // The option takes precedence over the environment variable and the config.
        assert_eq!(
            resolve_protocol_type(Some(ImageProtocolType::Iterm), env(), config).unwrap(),
            Some(ImageProtocolType::Iterm)
        );
        // The environment variable takes precedence over the config.
        assert_eq!(
            resolve_protocol_type(None, env(), config).unwrap(),
            Some(ImageProtocolType::KittyUnicode)
        );
        // Without the option and the environment variable, the config is used.
        assert_eq!(
            resolve_protocol_type(None, Ok(None), config).unwrap(),
            Some(ImageProtocolType::Kitty)
        );
        // Without any of them, the default is used.
        assert_eq!(resolve_protocol_type(None, Ok(None), None).unwrap(), None);
    }

    #[test]
    fn test_resolve_protocol_type_reports_an_invalid_environment_variable() {
        // A typo in the environment is reported even when the option takes precedence over it.
        let err = resolve_protocol_type(
            Some(ImageProtocolType::Ascii),
            protocol_type_from_value(Some("invalid".into())),
            None,
        )
        .unwrap_err();
        assert!(err.to_string().contains("VINE_PROTOCOL"), "{err}");

        let err =
            resolve_protocol_type(None, protocol_type_from_value(Some("invalid".into())), None)
                .unwrap_err();
        assert!(err.to_string().contains("VINE_PROTOCOL"), "{err}");
    }

    #[test]
    fn test_the_default_protocol_is_ascii() {
        // Without the option, the environment variable, and the config, every terminal renders the
        // ASCII graph: the image protocols have to be requested explicitly.
        let default = resolve_protocol_type(None, Ok(None), None).unwrap();
        assert_eq!(
            protocol::ImageProtocol::from(default),
            protocol::ImageProtocol::Ascii
        );

        // The explicit `auto` still detects the protocol of the current terminal.
        assert_eq!(
            protocol::ImageProtocol::from(Some(ImageProtocolType::Auto)),
            protocol::auto_detect()
        );
        assert_eq!(
            protocol::ImageProtocol::from(Some(ImageProtocolType::Ascii)),
            protocol::ImageProtocol::Ascii
        );
    }
}
