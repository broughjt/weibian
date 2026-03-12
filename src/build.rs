use std::collections::HashMap;
use std::fs;

use anyhow::anyhow;
use ecow::EcoVec;
use termcolor::StandardStream;
use typst::diag::{SourceDiagnostic, Warned};
use typst_kit::{
    diagnostics::{emit, DiagnosticFormat},
    downloader::SystemDownloader,
    files::{FsRoot, SystemFiles},
    packages::SystemPackages,
};
use typst_syntax::{FileId, RootedPath, VirtualPath, VirtualRoot};
use walkdir::WalkDir;

use crate::{
    compile::compile,
    config::BuildConfig,
    file_store::FileStore,
    world::{Resources, SystemWorld},
};

const USER_AGENT: &str = "weibian";

pub struct BuildState {
    file_store: FileStore<SystemFiles>,
    resources: Resources,
    config: BuildConfig,
}

impl BuildState {
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

    /// Compiles all source files and returns per-file diagnostics.
    ///
    /// Fatal errors (I/O failures, bad configuration) are returned as `Err`.
    /// Compilation warnings and errors are collected into the returned map and
    /// do not cause early termination. HTML is written for every file that
    /// compiles successfully, regardless of whether other files fail.
    pub fn build(
        &self,
    ) -> anyhow::Result<HashMap<FileId, (EcoVec<SourceDiagnostic>, EcoVec<SourceDiagnostic>)>> {
        if self.config.output_directory.exists() {
            fs::remove_dir_all(&self.config.output_directory)?;
        }
        fs::create_dir(&self.config.output_directory)?;

        let mut diagnostics: HashMap<FileId, (EcoVec<SourceDiagnostic>, EcoVec<SourceDiagnostic>)> =
            HashMap::new();

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

        for (counter, result) in (0_u32..).zip(paths) {
            let path = result?;
            let virtual_path = VirtualPath::virtualize(&self.config.root, &path)
                .map_err(|e| anyhow!("failed to virtualize path: {e:?}"))?;
            let id = FileId::new(RootedPath::new(VirtualRoot::Project, virtual_path));
            let world = SystemWorld::new(id, &self.resources, &self.file_store);

            let Warned {
                output: result,
                warnings,
            } = compile(&world);

            match result {
                Ok((_document, content)) => {
                    let output_path = self.config.output_directory.join(format!("{counter}.html"));
                    fs::write(&output_path, content)?;
                    if !warnings.is_empty() {
                        diagnostics.insert(id, (warnings, EcoVec::new()));
                    }
                }
                Err(errors) => {
                    diagnostics.insert(id, (warnings, errors));
                }
            }
        }

        Ok(diagnostics)
    }

    /// Emits all collected diagnostics to stderr using typst-kit's formatter.
    ///
    /// Returns `true` if any errors were present.
    pub fn emit_diagnostics(
        &self,
        stream: &mut StandardStream,
        diagnostics: &HashMap<FileId, (EcoVec<SourceDiagnostic>, EcoVec<SourceDiagnostic>)>,
    ) -> anyhow::Result<bool> {
        let mut has_error = false;

        for (&id, (warnings, errors)) in diagnostics {
            let world = SystemWorld::new(id, &self.resources, &self.file_store);
            let diagnostics = warnings.iter().chain(errors.iter());

            emit(stream, &world, diagnostics, DiagnosticFormat::Human)?;

            if !errors.is_empty() {
                has_error = true;
            }
        }

        Ok(has_error)
    }
}
