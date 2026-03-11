use std::fmt;
use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueHint};
use ecow::eco_format;
use figment::Figment;
use figment::providers::{Format, Toml};
use globset::{Glob, GlobSet, GlobSetBuilder};
use serde::de::{self, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};
use walkdir::WalkDir;

use typst::diag::StrResult;

const DEFAULT_CONFIG_NAME: &str = "weibian.toml";

/// The overall structure of the help.
#[rustfmt::skip]
const HELP_TEMPLATE: &str = "\
Weibian (wb) {version}

{usage-heading} {usage}

{all-args}{after-help}\
";

/// The Weibian CLI.
#[derive(Debug, Clone, Parser)]
#[clap(
    name = "wb",
    version = env!("CARGO_PKG_VERSION"),
    author,
    help_template = HELP_TEMPLATE,
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
    /// Compiles the source files to HTML.
    #[command(visible_alias = "c")]
    Compile,

    /// Watches the source files and recompiles on changes.
    #[command(visible_alias = "w")]
    Watch,
}

/// The raw deserialized contents of `weibian.toml`.
#[derive(Debug, Default, Deserialize)]
pub struct WeibianConfig {
    pub output_directory: Option<PathBuf>,

    #[serde(default, deserialize_with = "deserialize_globset")]
    pub include: GlobSet,

    #[serde(default, deserialize_with = "deserialize_globset")]
    pub exclude: GlobSet,
}

/// The fully resolved build configuration.
#[derive(Debug, Clone)]
pub struct BuildConfig {
    pub root: PathBuf,
    pub output_directory: PathBuf,
    pub include: GlobSet,
    pub exclude: GlobSet,
}

impl BuildConfig {
    pub fn try_load(config_file: Option<PathBuf>) -> StrResult<Self> {
        let (root, config) = load_config(config_file)?;

        let include = if config.include.is_empty() {
            GlobSetBuilder::new()
                .add(Glob::new("**/*.typ").map_err(|e| eco_format!("{e}"))?)
                .build()
                .map_err(|e| eco_format!("{e}"))?
        } else {
            config.include
        };

        let output_directory = config
            .output_directory
            .unwrap_or_else(|| PathBuf::from("dist"));

        Ok(Self {
            root,
            output_directory,
            include,
            exclude: config.exclude,
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
                        && self.include.is_match(relative)
                        && !self.exclude.is_match(relative)
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

fn load_config(config_file: Option<PathBuf>) -> StrResult<(PathBuf, WeibianConfig)> {
    match config_file {
        Some(path) => {
            if !path.exists() {
                return Err(eco_format!("config file {} does not exist", path.display()));
            }
            let root = path
                .parent()
                .ok_or_else(|| eco_format!("config path {} has no parent", path.display()))?
                .to_path_buf();
            let config = Figment::new()
                .merge(Toml::file(&path))
                .extract::<WeibianConfig>()
                .map_err(|err| eco_format!("failed to load config {}: {err}", path.display()))?;

            Ok((root, config))
        }
        None => {
            let root = find_project_root()?;
            let config_path = root.join(DEFAULT_CONFIG_NAME);
            let config = Figment::new()
                .merge(Toml::file(&config_path))
                .extract::<WeibianConfig>()
                .map_err(|err| {
                    eco_format!("failed to load config {}: {err}", config_path.display())
                })?;

            Ok((root, config))
        }
    }
}

fn find_project_root() -> StrResult<PathBuf> {
    let cwd = std::env::current_dir()
        .map_err(|err| eco_format!("failed to get current directory: {err}"))?;

    let mut directory = cwd.as_path();
    loop {
        if directory.join(DEFAULT_CONFIG_NAME).exists() {
            return Ok(directory.to_path_buf());
        }
        match directory.parent() {
            Some(parent) => directory = parent,
            None => {
                return Err(eco_format!(
                    "could not find {} in {} or any parent directory",
                    DEFAULT_CONFIG_NAME,
                    cwd.display()
                ));
            }
        }
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
