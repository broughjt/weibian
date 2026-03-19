use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::{fmt, fs};

use anyhow::anyhow;
use clap::{ArgAction, Parser, Subcommand, ValueHint, builder::ValueParser};
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

    /// Pass a KEY=VALUE input to Typst via sys.inputs.
    #[arg(
        long = "input",
        value_name = "KEY=VALUE",
        action = ArgAction::Append,
        value_parser = ValueParser::new(parse_system_input_pair),
        global = true,
    )]
    pub inputs: Vec<(String, String)>,

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
    #[serde(default)]
    pub files: FilesConfig,

    #[serde(default)]
    pub templates: TemplatesConfig,

    #[serde(default)]
    pub site: SiteConfig,

    #[serde(default)]
    pub inputs: HashMap<String, String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct FilesConfig {
    pub input_directory: Option<PathBuf>,
    pub output_directory: Option<PathBuf>,
    pub public_directory: Option<PathBuf>,

    #[serde(default, deserialize_with = "deserialize_globset")]
    pub include: GlobSet,

    #[serde(default, deserialize_with = "deserialize_globset")]
    pub exclude: GlobSet,
}

#[derive(Debug, Deserialize)]
pub struct TemplatesConfig {
    pub node: PathBuf,
    pub transclusion: PathBuf,
    pub link: PathBuf,
}

impl Default for TemplatesConfig {
    fn default() -> Self {
        Self {
            node: PathBuf::from(NODE_TEMPLATE),
            transclusion: PathBuf::from(TRANSCLUSION_TEMPLATE),
            link: PathBuf::from(LINK_TEMPLATE),
        }
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct SiteConfig {
    pub root_directory: Option<String>,
    pub trailing_slash: Option<bool>,
    pub index_node: Option<String>,
    pub domain: Option<String>,
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
    pub root_directory: String,
    pub trailing_slash: bool,
    pub index_node: String,
    pub domain: String,
    pub inputs: HashMap<String, String>,
    pub environment: minijinja::Environment<'static>,
}

impl BuildConfig {
    pub fn try_load(
        config_file: Option<PathBuf>,
        cli_inputs: Vec<(String, String)>,
    ) -> anyhow::Result<Self> {
        let (root, config) = load_config(config_file)?;

        let input_directory = config
            .files
            .input_directory
            .map(|p| if p.is_absolute() { p } else { root.join(p) })
            .unwrap_or_else(|| root.clone());

        let public_directory = config
            .files
            .public_directory
            .map(|p| if p.is_absolute() { p } else { root.join(p) });

        let include = if config.files.include.is_empty() {
            GlobSetBuilder::new().add(Glob::new("**/*.typ")?).build()?
        } else {
            config.files.include
        };

        let output_directory = config
            .files
            .output_directory
            .unwrap_or_else(|| root.join("dist"));

        let root_directory = normalize_root_directory(config.site.root_directory.as_deref());
        let trailing_slash = config.site.trailing_slash.unwrap_or(false);
        let index_node = config
            .site
            .index_node
            .unwrap_or_else(|| "index".to_string());
        let domain = config.site.domain.unwrap_or_default();

        let mut inputs: HashMap<String, String> = config.inputs;

        inputs.extend(cli_inputs);

        // Compiler-generated wb-* keys always win.
        inputs.insert("wb-domain".into(), domain.clone());
        inputs.insert("wb-root-directory".into(), root_directory.clone());
        inputs.insert("wb-trailing-slash".into(), trailing_slash.to_string());
        inputs.insert("wb-target".into(), "html".into());

        let node_template_path = if config.templates.node.is_absolute() {
            config.templates.node
        } else {
            root.join(config.templates.node)
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

        let transclusion_template_path = if config.templates.transclusion.is_absolute() {
            config.templates.transclusion
        } else {
            root.join(config.templates.transclusion)
        };
        let transclusion_template_source = fs::read_to_string(&transclusion_template_path)
            .map_err(|e| {
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

        let link_template_path = if config.templates.link.is_absolute() {
            config.templates.link
        } else {
            root.join(config.templates.link)
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

        environment.add_filter("demote_headings", filter_demote_headings);

        Ok(Self {
            root,
            input_directory,
            output_directory,
            public_directory,
            include,
            exclude: config.files.exclude,
            root_directory,
            trailing_slash,
            index_node,
            domain,
            inputs,
            environment,
        })
    }

    pub fn href(&self, id: &str) -> String {
        if self.trailing_slash {
            format!("{}{id}/", self.root_directory)
        } else {
            format!("{}{id}.html", self.root_directory)
        }
    }

    pub fn output_path(&self, id: &str) -> PathBuf {
        if self.trailing_slash && id != self.index_node {
            self.output_directory.join(id).join("index.html")
        } else {
            self.output_directory.join(format!("{id}.html"))
        }
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

pub fn copy_directory_recursive(
    src: &std::path::Path,
    dest: &std::path::Path,
) -> anyhow::Result<()> {
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

fn filter_demote_headings(html: String, levels: Option<u32>) -> String {
    demote_headings_html(html, levels.unwrap_or(1) as usize)
}

// TODO: move this somewhere more appropriate
fn demote_headings_html(html: String, levels: usize) -> String {
    if levels == 0 {
        return html;
    }

    let document = dom_query::Document::from(html.as_str());

    for n in (1u8..=6).rev() {
        let m = (n as usize + levels).min(6) as u8;
        if m == n {
            continue;
        }

        let selection = document.select(&format!("h{n}"));

        selection.rename(&format!("h{m}"));
        selection.set_attr("data-demoted", &levels.to_string());
    }

    document.select("body").inner_html().to_string()
}

fn parse_system_input_pair(s: &str) -> Result<(String, String), clap::Error> {
    s.split_once('=')
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .ok_or_else(|| {
            clap::Error::raw(
                clap::error::ErrorKind::ValueValidation,
                format!("--input value must be KEY=VALUE, got: {s:?}\n"),
            )
        })
}

fn normalize_root_directory(raw: Option<&str>) -> String {
    let mut root = raw
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("/")
        .to_string();
    if !root.starts_with('/') {
        root.insert(0, '/');
    }
    if !root.ends_with('/') {
        root.push('/');
    }

    root
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
