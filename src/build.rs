use std::fs;

use anyhow::anyhow;
use termcolor::{ColorChoice, StandardStream};
use typst_kit::{
    downloader::SystemDownloader,
    files::{FsRoot, SystemFiles},
    packages::SystemPackages,
};
use typst_syntax::{FileId, RootedPath, VirtualPath, VirtualRoot};
use walkdir::WalkDir;

use crate::{
    compiler::Compiler,
    config::{BuildConfig, copy_directory_recursive},
    file_store::FileStore,
    world::{Resources, SystemWorld},
};

use typst_kit::diagnostics::{DiagnosticFormat, emit};

const USER_AGENT: &str = "weibian";

pub struct Builder {
    file_store: FileStore<SystemFiles>,
    resources: Resources,
    config: BuildConfig,
}

impl Builder {
    /// Creates a `Builder` from a build configuration.
    pub fn new(config: BuildConfig) -> Self {
        let downloader = SystemDownloader::new(USER_AGENT);
        let packages = SystemPackages::new(downloader);
        let file_loader = SystemFiles::new(FsRoot::new(config.input_directory.clone()), packages);
        let file_store = FileStore::new(file_loader);
        let resources = Resources::new(&config.inputs);

        Self {
            file_store,
            resources,
            config,
        }
    }

    /// Compiles all source files, writes node HTML, and emits diagnostics to stderr.
    ///
    /// Fatal errors (I/O failures, bad configuration) are returned as `Err`.
    /// Returns `true` if any compilation errors were present.
    pub fn build(&self) -> anyhow::Result<bool> {
        if self.config.output_directory.exists() {
            fs::remove_dir_all(&self.config.output_directory)?;
        }
        fs::create_dir(&self.config.output_directory)?;

        if let Some(public_directory) = &self.config.public_directory {
            copy_directory_recursive(public_directory, &self.config.output_directory)?;
        }

        let mut compiler = Compiler::default();

        let ids = WalkDir::new(&self.config.input_directory)
            .into_iter()
            .filter_map(|result| match result {
                Ok(entry) => {
                    if entry.file_type().is_file() && self.config.is_match(entry.path()) {
                        let path = entry.into_path();
                        let result = VirtualPath::virtualize(&self.config.input_directory, &path)
                            .map(|virtual_path| {
                                FileId::new(RootedPath::new(VirtualRoot::Project, virtual_path))
                            })
                            .map_err(|error| anyhow!("failed to virtualize path: {error:?}"));

                        Some(result)
                    } else {
                        None
                    }
                }
                Err(e) => Some(Err(e.into())),
            });

        for result in ids {
            let id = result?;
            let world = SystemWorld::new(id, &self.resources, &self.file_store);

            compiler.compile(&world, id);
        }

        compiler.process(&self.config)?.apply(&self.config)?;

        let mut stderr = StandardStream::stderr(ColorChoice::Auto);
        let has_errors = self.emit_diagnostics(&mut stderr, &compiler)?;

        Ok(has_errors)
    }

    /// Emits all diagnostics from `compiler` to `stream`.
    ///
    /// Returns `true` if any errors were present.
    fn emit_diagnostics(
        &self,
        stream: &mut StandardStream,
        compiler: &Compiler,
    ) -> anyhow::Result<bool> {
        let mut has_errors = false;

        for (&id, (warnings, errors)) in compiler.compile_diagnostics() {
            let world = SystemWorld::new(id, &self.resources, &self.file_store);
            emit(
                stream,
                &world,
                warnings.iter().chain(errors.iter()),
                DiagnosticFormat::Human,
            )?;
            if !errors.is_empty() {
                has_errors = true;
            }
        }

        for (&id, errors) in compiler.process_diagnostics() {
            let world = SystemWorld::new(id, &self.resources, &self.file_store);
            emit(stream, &world, errors.iter(), DiagnosticFormat::Human)?;
            has_errors = true;
        }

        Ok(has_errors)
    }
}
