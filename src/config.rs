use std::path::{Path, PathBuf};
use std::{fmt, fs};

use anyhow::anyhow;
use clap::{Parser, Subcommand, ValueHint};
use figment::Figment;
use figment::providers::{Format, Toml};
use globset::{Glob, GlobSet, GlobSetBuilder};
use serde::de::{self, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};

pub const NODE_TEMPLATE: &str = "node.html";
pub const TRANSCLUSION_TEMPLATE: &str = "transclusion.html";
pub const LINK_TEMPLATE: &str = "link.html";

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
    /// Builds the static site.
    #[command(visible_alias = "b")]
    Build,

    /// Watches source files and recompiles on changes.
    #[command(visible_alias = "w")]
    Watch,
}

/// The raw deserialized contents of `weibian.toml`.
#[derive(Debug, Deserialize)]
pub struct WeibianConfig {
    pub input_directory: Option<PathBuf>,
    pub output_directory: Option<PathBuf>,
    pub node_template: PathBuf,
    pub transclusion_template: PathBuf,
    pub link_template: PathBuf,
    pub public_directory: Option<PathBuf>,

    #[serde(default, deserialize_with = "deserialize_globset")]
    pub include: GlobSet,

    #[serde(default, deserialize_with = "deserialize_globset")]
    pub exclude: GlobSet,
}

/// The fully resolved build configuration.
#[derive(Debug)]
pub struct BuildConfig {
    /// The directory of the `weibian.toml` file.
    pub root: PathBuf,
    /// The Typst project root and file scan root. Defaults to `root`.
    pub input_directory: PathBuf,
    pub output_directory: PathBuf,
    pub public_directory: Option<PathBuf>,
    pub include: GlobSet,
    pub exclude: GlobSet,
    pub environment: minijinja::Environment<'static>,
}

impl BuildConfig {
    pub fn try_load(config_file: Option<PathBuf>) -> anyhow::Result<Self> {
        let (root, config) = load_config(config_file)?;

        let input_directory = config
            .input_directory
            .map(|p| if p.is_absolute() { p } else { root.join(p) })
            .unwrap_or_else(|| root.clone());

        let public_directory = config
            .public_directory
            .map(|p| if p.is_absolute() { p } else { root.join(p) });

        let include = if config.include.is_empty() {
            GlobSetBuilder::new().add(Glob::new("**/*.typ")?).build()?
        } else {
            config.include
        };

        let output_directory = config.output_directory.unwrap_or_else(|| root.join("dist"));

        let node_template_path = if config.node_template.is_absolute() {
            config.node_template
        } else {
            root.join(config.node_template)
        };
        let node_template_source = fs::read_to_string(&node_template_path).map_err(|e| {
            anyhow!(
                "failed to read node template {}: {e}",
                node_template_path.display()
            )
        })?;
        let mut environment = minijinja::Environment::new();

        environment
            .add_template_owned(NODE_TEMPLATE, node_template_source)
            .map_err(|e| {
                anyhow!(
                    "failed to parse node template {}: {e}",
                    node_template_path.display()
                )
            })?;

        let transclusion_template_path = if config.transclusion_template.is_absolute() {
            config.transclusion_template
        } else {
            root.join(config.transclusion_template)
        };
        let transclusion_template_source =
            fs::read_to_string(&transclusion_template_path).map_err(|e| {
                anyhow!(
                    "failed to read transclusion template {}: {e}",
                    transclusion_template_path.display()
                )
            })?;
        environment
            .add_template_owned(TRANSCLUSION_TEMPLATE, transclusion_template_source)
            .map_err(|e| {
                anyhow!(
                    "failed to parse transclusion template {}: {e}",
                    transclusion_template_path.display()
                )
            })?;

        let link_template_path = if config.link_template.is_absolute() {
            config.link_template
        } else {
            root.join(config.link_template)
        };
        let link_template_source = fs::read_to_string(&link_template_path).map_err(|e| {
            anyhow!(
                "failed to read link template {}: {e}",
                link_template_path.display()
            )
        })?;
        environment
            .add_template_owned(LINK_TEMPLATE, link_template_source)
            .map_err(|e| {
                anyhow!(
                    "failed to parse link template {}: {e}",
                    link_template_path.display()
                )
            })?;

        Ok(Self {
            root,
            input_directory,
            output_directory,
            public_directory,
            include,
            exclude: config.exclude,
            environment,
        })
    }

    pub fn is_match(&self, path: &Path) -> bool {
        path.strip_prefix(&self.input_directory)
            .ok()
            .is_some_and(|relative| {
                self.include.is_match(relative) && !self.exclude.is_match(relative)
            })
    }
}

fn load_config(config_file: Option<PathBuf>) -> anyhow::Result<(PathBuf, WeibianConfig)> {
    let (root, config_path) = match config_file {
        Some(path) => {
            if !path.exists() {
                return Err(anyhow!("config file {} does not exist", path.display()));
            }
            let root = path
                .parent()
                .ok_or_else(|| anyhow!("config path {} has no parent", path.display()))?
                .to_path_buf();

            (root, path)
        }
        None => {
            let root = find_project_root()?;
            let config_path = root.join(DEFAULT_CONFIG_NAME);

            (root, config_path)
        }
    };

    let config = Figment::new()
        .merge(Toml::file(&config_path))
        .extract::<WeibianConfig>()
        .map_err(|err| anyhow!("failed to load config {}: {err}", config_path.display()))?;

    Ok((root, config))
}

fn find_project_root() -> anyhow::Result<PathBuf> {
    let cwd = std::env::current_dir()
        .map_err(|error| anyhow::Error::from(error).context("Failed to get current directory"))?;

    let mut directory = cwd.as_path();
    loop {
        if directory.join(DEFAULT_CONFIG_NAME).exists() {
            return Ok(directory.to_path_buf());
        }
        match directory.parent() {
            Some(parent) => directory = parent,
            None => {
                return Err(anyhow!(
                    "could not find {} in {} or any parent directory",
                    DEFAULT_CONFIG_NAME,
                    cwd.display()
                ));
            }
        }
    }
}

pub fn copy_directory_recursive(src: &std::path::Path, dest: &std::path::Path) -> anyhow::Result<()> {
    use walkdir::WalkDir;

    for entry in WalkDir::new(src) {
        let entry = entry?;
        let rel = entry.path().strip_prefix(src)?;
        let target = dest.join(rel);
        if entry.file_type().is_dir() {
            std::fs::create_dir_all(&target)?;
        } else {
            std::fs::copy(entry.path(), &target)?;
        }
    }

    Ok(())
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
