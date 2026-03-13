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
    config::BuildConfig,
    file_store::FileStore,
    world::{Resources, SystemWorld},
};

const USER_AGENT: &str = "weibian";

pub struct Builder {
    file_store: FileStore<SystemFiles>,
    resources: Resources,
    config: BuildConfig,
}

impl Builder {
    pub fn new(config: BuildConfig) -> Self {
        let downloader = SystemDownloader::new(USER_AGENT);
        let packages = SystemPackages::new(downloader);
        let file_loader = SystemFiles::new(FsRoot::new(config.root.clone()), packages);
        let file_store = FileStore::new(file_loader);
        let resources = Resources::default();

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

        let mut compiler = Compiler::new();

        let paths = WalkDir::new(&self.config.root)
            .into_iter()
            .filter_map(|result| match result {
                Ok(entry) => {
                    if entry.file_type().is_file() && self.config.is_match(entry.path()) {
                        Some(Ok(entry.into_path()))
                    } else {
                        None
                    }
                }
                Err(e) => Some(Err(e)),
            });

        for result in paths {
            let path = result?;
            let virtual_path = VirtualPath::virtualize(&self.config.root, &path)
                .map_err(|e| anyhow!("failed to virtualize path: {e:?}"))?;
            let id = FileId::new(RootedPath::new(VirtualRoot::Project, virtual_path));
            let world = SystemWorld::new(id, &self.resources, &self.file_store);

            compiler.compile(&world, id)?;
        }

        compiler.process(&self.config.output_directory)?;

        let mut stderr = StandardStream::stderr(ColorChoice::Auto);
        let has_errors =
            compiler.emit_diagnostics(&mut stderr, &self.file_store, &self.resources)?;

        Ok(has_errors)
    }
}
