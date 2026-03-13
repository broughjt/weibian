use std::collections::HashSet;
use std::fs;
use std::io::{IsTerminal, Write};
use std::sync::mpsc;
use std::time::Duration;

use anyhow::anyhow;
use notify_debouncer_full::notify::{EventKind, RecursiveMode, event::ModifyKind};
use notify_debouncer_full::{DebounceEventResult, new_debouncer};
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
    import_graph::ImportGraph,
    world::{DependenciesWorld, Resources, SystemWorld},
};

// TODO: Deduplicate with build.rs (but not yet)
const USER_AGENT: &str = "weibian";

const DEBOUNCE_TIMEOUT: Duration = Duration::from_millis(500);

pub struct WatchState {
    file_store: FileStore<SystemFiles>,
    resources: Resources,
    import_graph: ImportGraph,
    compiler: Compiler,
    config: BuildConfig,
}

impl WatchState {
    pub fn new(config: BuildConfig) -> Self {
        let downloader = SystemDownloader::new(USER_AGENT);
        let packages = SystemPackages::new(downloader);
        let file_loader = SystemFiles::new(FsRoot::new(config.root.clone()), packages);
        let file_store = FileStore::new(file_loader);

        Self {
            file_store,
            resources: Resources::default(),
            import_graph: ImportGraph::default(),
            compiler: Compiler::new(),
            config,
        }
    }

    /// Compiles a single source file, updating the import graph and compiler
    /// state (node store and diagnostics).
    ///
    /// Fatal errors (I/O failures) are returned as `Err`. Compilation warnings
    /// and errors are recorded in the compiler and do not cause early
    /// termination.
    pub fn build(&mut self, id: FileId) -> anyhow::Result<()> {
        let world = DependenciesWorld::new(SystemWorld::new(id, &self.resources, &self.file_store));

        self.compiler.compile(&world, id)?;

        let (_, dependencies) = world.into_inner();

        self.import_graph.update(id, dependencies);

        Ok(())
    }

    fn emit_diagnostics(&self) -> anyhow::Result<()> {
        let mut stderr = StandardStream::stderr(ColorChoice::Auto);

        if std::io::stderr().is_terminal() {
            write!(stderr, "\x1B[2J\x1B[1;1H")?;
            stderr.flush()?;
        }

        self.compiler
            .emit_diagnostics(&mut stderr, &self.file_store, &self.resources)?;

        Ok(())
    }

    pub fn watch(&mut self) -> anyhow::Result<()> {
        if self.config.output_directory.exists() {
            fs::remove_dir_all(&self.config.output_directory)?;
        }
        fs::create_dir(&self.config.output_directory)?;

        for result in WalkDir::new(&self.config.root) {
            let entry = result?;
            if !entry.file_type().is_file() || !self.config.is_match(entry.path()) {
                continue;
            }
            let virtual_path = VirtualPath::virtualize(&self.config.root, entry.path())
                .map_err(|e| anyhow!("failed to virtualize path: {e:?}"))?;
            let id = FileId::new(RootedPath::new(VirtualRoot::Project, virtual_path));
            self.build(id)?;
        }
        self.compiler.process(&self.config.output_directory)?;
        self.emit_diagnostics()?;

        let (sender, receiver) = mpsc::channel::<DebounceEventResult>();

        let mut debouncer = new_debouncer(DEBOUNCE_TIMEOUT, None, sender)
            .map_err(|e| anyhow::Error::from(e).context("Failed to create file watcher"))?;

        debouncer
            .watch(&self.config.root, RecursiveMode::Recursive)
            .map_err(|e| anyhow::anyhow!("failed to watch {}: {e}", self.config.root.display()))?;

        let mut recompile: HashSet<FileId> = HashSet::new();

        for result in receiver {
            match result {
                Ok(events) => {
                    for event in events {
                        if matches!(
                            event.kind,
                            EventKind::Access(_)
                                | EventKind::Other
                                | EventKind::Modify(ModifyKind::Metadata(_))
                                | EventKind::Modify(ModifyKind::Other)
                        ) {
                            continue;
                        }

                        for path in &event.paths {
                            if path.starts_with(&self.config.output_directory) {
                                continue;
                            }

                            let virtual_path =
                                match VirtualPath::virtualize(&self.config.root, path) {
                                    Ok(virtual_path) => virtual_path,
                                    Err(e) => {
                                        eprintln!(
                                            "watch: failed to virtualize {}: {e:?}",
                                            path.display()
                                        );
                                        continue;
                                    }
                                };
                            let id =
                                FileId::new(RootedPath::new(VirtualRoot::Project, virtual_path));

                            match event.kind {
                                EventKind::Create(_) | EventKind::Modify(_) => {
                                    // If a source file or one of its dependencies has been updated:
                                    //
                                    // - Compute dependencies of this file
                                    // - Reset all of them
                                    // - Add them to the set of files to be recompiled
                                    // - If file itself is a source file, add it to the set of files to be recompiled
                                    let dependents = self.import_graph.dependents(id);

                                    self.file_store.reset(
                                        std::iter::once(id).chain(dependents.iter().copied()),
                                    );

                                    if self.config.is_match(path) {
                                        recompile.insert(id);
                                    }

                                    recompile.extend(dependents);
                                }
                                EventKind::Remove(_) => {
                                    // If a source file or one of its dependencies has been removed:
                                    //
                                    // - Compute dependencies
                                    // - Reset all of them
                                    //
                                    let dependents = self.import_graph.dependents(id);

                                    self.file_store.reset(
                                        std::iter::once(id).chain(dependents.iter().copied()),
                                    );

                                    self.import_graph.remove(id);
                                    if self.config.is_match(path) {
                                        self.compiler.remove(id);
                                    }

                                    recompile.extend(dependents);
                                }
                                _ => {}
                            }
                        }
                    }

                    for id in recompile.drain() {
                        self.build(id)?;
                    }

                    if self.config.output_directory.exists() {
                        fs::remove_dir_all(&self.config.output_directory)?;
                    }
                    fs::create_dir(&self.config.output_directory)?;

                    self.compiler.process(&self.config.output_directory)?;
                    self.emit_diagnostics()?;

                    comemo::evict(10);
                }
                Err(errors) => {
                    for error in errors {
                        eprintln!("watch error: {error}");
                    }
                }
            }
        }

        Ok(())
    }
}
