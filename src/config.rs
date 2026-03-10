use std::fmt::{self, Display, Formatter};
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use clap::builder::{BoolishValueParser, ValueParser};
use clap::{ArgAction, Args, Parser, Subcommand, ValueEnum, ValueHint};
use ecow::eco_format;
use figment::Figment;
use figment::providers::{Format, Toml};
use globset::{Glob, GlobSet, GlobSetBuilder};
use serde::de::{self, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};
use walkdir::WalkDir;

use typst::diag::StrResult;

/// The character typically used to separate path components
/// in environment variables.
const ENVIRONMENT_PATH_SEPARATOR: char = if cfg!(windows) { ';' } else { ':' };

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

const DEFAULT_CONFIG_PATH: &str = ".wb/config.toml";

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
pub struct Arguments {
    /// Path to a Weibian configuration file.
    #[arg(
        long = "config-file",
        value_name = "PATH",
        value_hint = ValueHint::FilePath,
        global = true
    )]
    pub config_file: Option<PathBuf>,

    /// The command to run.
    #[command(subcommand)]
    pub command: Command,
}

/// What to do.
#[derive(Debug, Clone, Subcommand)]
pub enum Command {
    /// Compiles the input directory to HTML.
    #[command(visible_alias = "c")]
    Compile(CompileArguments),

    /// Watches the input directory and recompiles on changes.
    #[command(visible_alias = "w")]
    Watch(CompileArguments),
}

/// Arguments for compilation and watching.
#[derive(Debug, Clone, Args)]
pub struct CompileArguments {
    /// Path to public assets directory (defaults to config or "public").
    #[clap(long = "public-dir", value_hint = ValueHint::DirPath)]
    pub public: Option<PathBuf>,

    /// Path to output directory (defaults to config or "dist").
    #[clap(value_hint = ValueHint::DirPath)]
    pub output: Option<PathBuf>,

    /// Site configuration.
    #[clap(flatten)]
    pub site: SiteArguments,

    /// World arguments.
    #[clap(flatten)]
    pub world: WorldArguments,

    /// One (or multiple comma-separated) PDF standards that Typst will enforce
    /// conformance with.
    #[arg(long = "pdf-standard", value_delimiter = ',')]
    pub pdf_standard: Vec<PdfStandard>,
}

/// Site configuration overrides.
#[derive(Debug, Clone, Args)]
pub struct SiteArguments {
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
pub struct WorldArguments {
    /// Configures the project root (for absolute paths).
    #[clap(long = "root", env = "WEIBIAN_ROOT", value_name = "DIR", value_hint = ValueHint::DirPath)]
    pub root: Option<PathBuf>,

    /// Add a string key-value pair visible through `sys.inputs`.
    #[clap(
        long = "input",
        value_name = "key=value",
        action = ArgAction::Append,
        value_parser = ValueParser::new(parse_key_value_pair),
    )]
    pub inputs: Vec<(String, String)>,

    /// Common font arguments.
    #[clap(flatten)]
    pub font: FontArguments,

    /// Arguments related to storage of packages in the system.
    #[clap(flatten)]
    pub package: PackageArguments,

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
pub struct PackageArguments {
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
pub struct FontArguments {
    /// Adds additional directories that are recursively searched for fonts.
    ///
    /// If multiple paths are specified, they are separated by the system's path
    /// separator (`:` on Unix-like systems and `;` on Windows).
    #[clap(
        long = "font-path",
        env = "TYPST_FONT_PATHS",
        value_name = "DIR",
        value_delimiter = ENVIRONMENT_PATH_SEPARATOR,
    )]
    pub font_paths: Vec<PathBuf>,

    /// Ensures system fonts won't be searched, unless explicitly included via
    /// `--font-path`.
    #[arg(long)]
    pub ignore_system_fonts: bool,
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

impl Display for PdfStandard {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        self.to_possible_value()
            .expect("no values are skipped")
            .get_name()
            .fmt(f)
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct WeibianConfig {
    #[serde(default, alias = "directories")]
    pub files: FilesConfig,

    #[serde(default)]
    pub site: SiteConfig,
}

#[derive(Debug, Default, Deserialize)]
pub struct FilesConfig {
    pub output_directory: Option<PathBuf>,
    pub public_directory: Option<PathBuf>,
    #[serde(default, deserialize_with = "deserialize_globset")]
    pub include: GlobSet,
    #[serde(default, deserialize_with = "deserialize_globset")]
    pub exclude: GlobSet,
}

#[derive(Debug, Default, Deserialize)]
pub struct SiteConfig {
    pub domain: Option<String>,
    pub root_directory: Option<String>,
    pub trailing_slash: Option<bool>,
}

#[derive(Debug, Clone)]
pub struct SiteSettings {
    pub domain: Option<String>,
    pub root_directory: String,
    pub trailing_slash: bool,
}

/// A preprocessed `CompileCommand` with config defaults applied.
#[derive(Debug, Clone)]
pub struct BuildConfig {
    pub root: PathBuf,
    pub include: GlobSet,
    pub exclude: GlobSet,
    pub public_directory: PathBuf,
    pub output_directory: PathBuf,
    pub site: SiteSettings,
    pub world: WorldArguments,
}

impl BuildConfig {
    pub fn try_load(arguments: Arguments) -> StrResult<Self> {
        let compile_arguments = match arguments.command {
            Command::Compile(arguments) | Command::Watch(arguments) => arguments,
        };
        let mut world = compile_arguments.world;
        let root = world.root.take().map_or_else(find_project_root, Ok)?;

        let config = {
            let (config_path, was_specified) = arguments
                .config_file
                .map(|p| (p, true))
                .unwrap_or_else(|| (root.join(DEFAULT_CONFIG_PATH), false));

            if config_path.exists() {
                Figment::new()
                    .merge(Toml::file(&config_path))
                    .extract::<WeibianConfig>()
                    .map_err(|err| {
                        eco_format!("failed to load config {}: {err}", config_path.display())
                    })?
            } else if was_specified {
                return Err(eco_format!(
                    "config file {} does not exist",
                    config_path.display()
                ));
            } else {
                WeibianConfig::default()
            }
        };
        let include = config.files.include;
        let exclude = config.files.exclude;
        let public_directory = compile_arguments
            .public
            .or(config.files.public_directory)
            .unwrap_or_else(|| PathBuf::from("public"));
        let output_directory = compile_arguments
            .output
            .or(config.files.output_directory)
            .unwrap_or_else(|| PathBuf::from("dist"));
        let domain = compile_arguments.site.domain.or(config.site.domain);
        let root_directory = {
            let s = compile_arguments
                .site
                .root_directory
                .as_deref()
                .or(config.site.root_directory.as_deref())
                .unwrap_or("/");
            let mut t = s.trim().to_owned();

            if !t.starts_with('/') {
                t.insert(0, '/');
            }
            if !t.ends_with('/') {
                t.push('/');
            }

            t
        };
        let trailing_slash = compile_arguments
            .site
            .trailing_slash
            .unwrap_or(config.site.trailing_slash.unwrap_or(false));

        Ok(Self {
            root,
            include,
            exclude,
            public_directory,
            output_directory,
            site: SiteSettings {
                domain,
                root_directory,
                trailing_slash,
            },
            world,
        })
    }

    pub fn iter_typst_sources(&self) -> impl Iterator<Item = Result<PathBuf, walkdir::Error>> {
        WalkDir::new(&self.root)
            .into_iter()
            .filter_map(|result| match result {
                Ok(entry) => {
                    let path = entry.path();
                    let relative = path.strip_prefix(&self.root).ok()?;

                    if entry.file_type().is_file()
                        && path.extension().is_some_and(|e| e == "typ")
                        && !self.exclude.is_match(relative)
                        && (self.include.is_empty() || self.include.is_match(relative))
                    {
                        Some(Ok(entry.into_path()))
                    } else {
                        None
                    }
                }
                Err(e) => Some(Err(e)),
            })
    }
}

fn deserialize_globset<'de, D>(deserializer: D) -> Result<GlobSet, D::Error>
where
    D: Deserializer<'de>,
{
    struct GlobSetVisitor;

    impl<'de> Visitor<'de> for GlobSetVisitor {
        type Value = GlobSet;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("a glob string or list of glob strings")
        }

        fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            let mut builder = GlobSetBuilder::new();
            builder.add(Glob::new(value).map_err(E::custom)?);
            builder.build().map_err(E::custom)
        }

        fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            self.visit_str(&value)
        }

        fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
        where
            A: SeqAccess<'de>,
        {
            let mut builder = GlobSetBuilder::new();
            while let Some(pattern) = seq.next_element::<String>()? {
                builder.add(Glob::new(&pattern).map_err(de::Error::custom)?);
            }
            builder.build().map_err(de::Error::custom)
        }
    }

    deserializer.deserialize_any(GlobSetVisitor)
}

fn parse_key_value_pair(raw: &str) -> Result<(String, String), String> {
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

fn parse_source_date_epoch(raw: &str) -> Result<DateTime<Utc>, String> {
    let timestamp: i64 = raw
        .parse()
        .map_err(|err| format!("timestamp must be decimal integer ({err})"))?;
    DateTime::from_timestamp(timestamp, 0).ok_or_else(|| "timestamp out of range".to_string())
}

fn find_project_root() -> StrResult<PathBuf> {
    let cwd = std::env::current_dir()
        .map_err(|err| eco_format!("failed to get current directory: {err}"))?;

    let mut directory = cwd.as_path();
    loop {
        if directory.join(".wb").is_dir() {
            return Ok(directory.to_path_buf());
        }
        match directory.parent() {
            Some(parent) => directory = parent,
            None => {
                return Err(eco_format!(
                    "could not find a .wb/ directory in {} or any parent directory",
                    cwd.display()
                ));
            }
        }
    }
}
