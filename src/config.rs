use std::fmt;
use std::path::PathBuf;

use ecow::eco_format;
use figment::Figment;
use figment::providers::{Format, Toml};
use globset::{Glob, GlobSet, GlobSetBuilder};
use serde::de::{self, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};
use walkdir::WalkDir;

use typst::diag::StrResult;

use crate::args::{CliArguments, Command, WorldArgs};

const DEFAULT_CONFIG_PATH: &str = ".wb/config.toml";

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
    pub world: WorldArgs,
}

impl BuildConfig {
    pub fn try_load(args: CliArguments) -> StrResult<Self> {
        let config_path = args
            .global
            .config_file
            .as_deref()
            .map(|p| (p.to_path_buf(), false))
            .unwrap_or_else(|| (PathBuf::from(DEFAULT_CONFIG_PATH), true));

        let config = if !config_path.0.exists() {
            if config_path.1 {
                WeibianConfig::default()
            } else {
                return Err(eco_format!(
                    "config file {} does not exist",
                    config_path.0.display()
                ));
            }
        } else {
            Figment::new()
                .merge(Toml::file(&config_path.0))
                .extract::<WeibianConfig>()
                .map_err(|err| eco_format!("failed to load config {}: {err}", config_path.0.display()))?
        };

        let compile_args = match args.command {
            Command::Compile(cmd) => cmd.args,
            Command::Watch(cmd) => cmd.args,
        };

        let mut world = compile_args.world;
        let root = match world.root.take() {
            Some(root) => root,
            None => find_project_root()?,
        };
        let include = config.files.include;
        let exclude = config.files.exclude;
        let public_directory = resolve_directory(
            compile_args.public.as_ref(),
            config.files.public_directory.as_ref(),
            "public",
        );
        let output_directory = resolve_directory(
            compile_args.output.as_ref(),
            config.files.output_directory.as_ref(),
            "dist",
        );

        let domain = compile_args.site.domain.or(config.site.domain);
        let root_directory = normalize_root_directory(
            compile_args.site.root_directory.as_deref().or(config.site.root_directory.as_deref()),
        );
        let trailing_slash = compile_args
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
        WalkDir::new(&self.root).into_iter().filter_map(|result| {
            match result {
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
            }
        })
    }
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

fn resolve_directory(cli: Option<&PathBuf>, config: Option<&PathBuf>, default: &str) -> PathBuf {
    cli.cloned()
        .or_else(|| config.cloned())
        .unwrap_or_else(|| PathBuf::from(default))
}

fn normalize_root_directory(raw: Option<&str>) -> String {
    let mut root = raw.unwrap_or("/").trim().to_string();
    if root.is_empty() {
        root = "/".to_string();
    }
    if !root.starts_with('/') {
        root.insert(0, '/');
    }
    if !root.ends_with('/') {
        root.push('/');
    }
    root
}
