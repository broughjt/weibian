use std::path::PathBuf;

use chrono::{DateTime, Utc};
use clap::builder::{BoolishValueParser, ValueParser};
use clap::{ArgAction, Args, Parser, Subcommand, ValueEnum, ValueHint};
use std::fmt::{self, Display, Formatter};

/// The character typically used to separate path components
/// in environment variables.
const ENV_PATH_SEP: char = if cfg!(windows) { ';' } else { ':' };

/// The overall structure of the help.
#[rustfmt::skip]
const HELP_TEMPLATE: &str = "\
Weibian (wb) {version}

{usage-heading} {usage}

{all-args}{after-help}\
";

/// Adds a list of useful links after the normal help.
#[rustfmt::skip]
const AFTER_HELP: &str = color_print::cstr!("\
<s>Repository:</>                 https://github.com/hanwenguo/weibian/
");

/// The Weibian CLI.
#[derive(Debug, Clone, Parser)]
#[clap(
    name = "wb",
    version = env!("CARGO_PKG_VERSION"),
    author,
    help_template = HELP_TEMPLATE,
    after_help = AFTER_HELP,
    max_term_width = 80,
)]
pub struct CliArguments {
    /// Global arguments.
    #[clap(flatten)]
    pub global: GlobalArgs,

    /// The command to run.
    #[command(subcommand)]
    pub command: Command,
}

/// Arguments shared by all commands.
#[derive(Debug, Clone, Args)]
pub struct GlobalArgs {
    /// Path to a Weibian configuration file.
    #[arg(
        long = "config-file",
        value_name = "PATH",
        value_hint = ValueHint::FilePath,
        global = true
    )]
    pub config_file: Option<PathBuf>,
}

/// What to do.
#[derive(Debug, Clone, Subcommand)]
pub enum Command {
    /// Compiles the input directory to HTML.
    #[command(visible_alias = "c")]
    Compile(CompileCommand),

    /// Watches the input directory and recompiles on changes.
    #[command(visible_alias = "w")]
    Watch(WatchCommand),
}

/// Compiles the input directory to HTML.
#[derive(Debug, Clone, Parser)]
pub struct CompileCommand {
    #[clap(flatten)]
    pub args: CompileArgs,
}

/// Watches the input directory and recompiles on changes.
#[derive(Debug, Clone, Parser)]
pub struct WatchCommand {
    #[clap(flatten)]
    pub args: CompileArgs,
}

/// Arguments for compilation and watching.
#[derive(Debug, Clone, Args)]
pub struct CompileArgs {
    /// Path to public assets directory (defaults to config or "public").
    #[clap(long = "public-dir", value_hint = ValueHint::DirPath)]
    pub public: Option<PathBuf>,

    /// Path to output directory (defaults to config or "dist").
    #[clap(value_hint = ValueHint::DirPath)]
    pub output: Option<PathBuf>,

    /// Site configuration.
    #[clap(flatten)]
    pub site: SiteArgs,

    /// World arguments.
    #[clap(flatten)]
    pub world: WorldArgs,

    /// One (or multiple comma-separated) PDF standards that Typst will enforce
    /// conformance with.
    #[arg(long = "pdf-standard", value_delimiter = ',')]
    pub pdf_standard: Vec<PdfStandard>,
}

/// Site configuration overrides.
#[derive(Debug, Clone, Args)]
pub struct SiteArgs {
    /// The domain of the site used for generating absolute URLs.
    #[arg(long = "site-domain", value_name = "DOMAIN")]
    pub domain: Option<String>,

    /// Root directory of the site (for example, "/notes/").
    #[arg(long = "site-root-dir", value_name = "DIR")]
    pub root_directory: Option<String>,

    /// Whether note URLs should end with a trailing slash.
    #[arg(
        long = "trailing-slash",
        value_parser = BoolishValueParser::new(),
        value_name = "BOOL"
    )]
    pub trailing_slash: Option<bool>,
}

/// Arguments for the Typst world.
#[derive(Debug, Clone, Args)]
pub struct WorldArgs {
    /// Configures the project root (for absolute paths).
    #[clap(long = "root", env = "WEIBIAN_ROOT", value_name = "DIR", value_hint = ValueHint::DirPath)]
    pub root: Option<PathBuf>,

    /// Add a string key-value pair visible through `sys.inputs`.
    #[clap(
        long = "input",
        value_name = "key=value",
        action = ArgAction::Append,
        value_parser = ValueParser::new(parse_sys_input_pair),
    )]
    pub inputs: Vec<(String, String)>,

    /// Common font arguments.
    #[clap(flatten)]
    pub font: FontArgs,

    /// Arguments related to storage of packages in the system.
    #[clap(flatten)]
    pub package: PackageArgs,

    /// The project's creation date formatted as a UNIX timestamp.
    ///
    /// For more information, see <https://reproducible-builds.org/specs/source-date-epoch/>.
    #[clap(
        long = "creation-timestamp",
        env = "SOURCE_DATE_EPOCH",
        value_name = "UNIX_TIMESTAMP",
        value_parser = parse_source_date_epoch,
    )]
    pub creation_timestamp: Option<DateTime<Utc>>,
}

/// Arguments related to where packages are stored in the system.
#[derive(Debug, Clone, Args)]
pub struct PackageArgs {
    /// Custom path to local packages, defaults to system-dependent location.
    #[clap(long = "package-path", env = "TYPST_PACKAGE_PATH", value_name = "DIR")]
    pub package_path: Option<PathBuf>,

    /// Custom path to package cache, defaults to system-dependent location.
    #[clap(
        long = "package-cache-path",
        env = "TYPST_PACKAGE_CACHE_PATH",
        value_name = "DIR"
    )]
    pub package_cache_path: Option<PathBuf>,
}

/// Common arguments to customize available fonts.
#[derive(Debug, Clone, Args)]
pub struct FontArgs {
    /// Adds additional directories that are recursively searched for fonts.
    ///
    /// If multiple paths are specified, they are separated by the system's path
    /// separator (`:` on Unix-like systems and `;` on Windows).
    #[clap(
        long = "font-path",
        env = "TYPST_FONT_PATHS",
        value_name = "DIR",
        value_delimiter = ENV_PATH_SEP,
    )]
    pub font_paths: Vec<PathBuf>,

    /// Ensures system fonts won't be searched, unless explicitly included via
    /// `--font-path`.
    #[arg(long)]
    pub ignore_system_fonts: bool,
}

macro_rules! display_possible_values {
    ($ty:ty) => {
        impl Display for $ty {
            fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
                self.to_possible_value()
                    .expect("no values are skipped")
                    .get_name()
                    .fmt(f)
            }
        }
    };
}

/// A PDF standard that Typst can enforce conformance with.
#[derive(Debug, Copy, Clone, Eq, PartialEq, ValueEnum)]
#[allow(non_camel_case_types)]
pub enum PdfStandard {
    /// PDF 1.7.
    #[value(name = "1.7")]
    V_1_7,
    /// PDF/A-2b.
    #[value(name = "a-2b")]
    A_2b,
    /// PDF/A-3b.
    #[value(name = "a-3b")]
    A_3b,
}

display_possible_values!(PdfStandard);

/// Parses key/value pairs split by the first equal sign.
///
/// This function will return an error if the argument contains no equals sign
/// or contains the key (before the equals sign) is empty.
fn parse_sys_input_pair(raw: &str) -> Result<(String, String), String> {
    let (key, val) = raw
        .split_once('=')
        .ok_or("input must be a key and a value separated by an equal sign")?;
    let key = key.trim().to_owned();
    if key.is_empty() {
        return Err("the key was missing or empty".to_owned());
    }
    let val = val.trim().to_owned();
    Ok((key, val))
}

/// Parses a UNIX timestamp according to <https://reproducible-builds.org/specs/source-date-epoch/>
fn parse_source_date_epoch(raw: &str) -> Result<DateTime<Utc>, String> {
    let timestamp: i64 = raw
        .parse()
        .map_err(|err| format!("timestamp must be decimal integer ({err})"))?;
    DateTime::from_timestamp(timestamp, 0).ok_or_else(|| "timestamp out of range".to_string())
}
