use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{IsTerminal, Write};
use std::sync::mpsc;
use std::time::Duration;

use anyhow::anyhow;
use ecow::EcoVec;
use notify_debouncer_full::notify::{event::ModifyKind, EventKind, RecursiveMode};
use notify_debouncer_full::{new_debouncer, DebounceEventResult};
use termcolor::{ColorChoice, StandardStream};
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
    outputs: HashMap<FileId, String>,
    diagnostics: HashMap<FileId, (EcoVec<SourceDiagnostic>, EcoVec<SourceDiagnostic>)>,
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
            outputs: HashMap::new(),
            diagnostics: HashMap::new(),
            config,
        }
    }

    /// Compiles a single source file, updating the import graph, outputs, and
    /// diagnostics maps.
    ///
    /// Fatal errors (I/O failures) are returned as `Err`. Compilation warnings
    /// and errors are recorded in `self.diagnostics` and do not cause early
    /// termination.
    pub fn build(&mut self, id: FileId) -> anyhow::Result<()> {
        let world = DependenciesWorld::new(SystemWorld::new(id, &self.resources, &self.file_store));

        let Warned {
            output: result,
            warnings,
        } = compile(&world);

        let (_, dependencies) = world.into_inner();
        self.import_graph.update(id, dependencies);

        match result {
            Ok((_, html)) => {
                let output_path = self
                    .config
                    .output_directory
                    .join(id.vpath().get_without_slash())
                    .with_extension("html");
                if let Some(parent) = output_path.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::write(&output_path, &html)?;
                self.outputs.insert(id, html);
                if !warnings.is_empty() {
                    self.diagnostics.insert(id, (warnings, EcoVec::new()));
                } else {
                    self.diagnostics.remove(&id);
                }
            }
            Err(errors) => {
                self.outputs.remove(&id);
                self.diagnostics.insert(id, (warnings, errors));
            }
        }

        Ok(())
    }

    fn emit_diagnostics(&self) -> anyhow::Result<()> {
        let mut stderr = StandardStream::stderr(ColorChoice::Auto);

        if std::io::stderr().is_terminal() {
            write!(stderr, "\x1B[2J\x1B[1;1H")?;
            stderr.flush()?;
        }

        for (&id, (warnings, errors)) in &self.diagnostics {
            let world = SystemWorld::new(id, &self.resources, &self.file_store);
            emit(
                &mut stderr,
                &world,
                warnings.iter().chain(errors.iter()),
                DiagnosticFormat::Human,
            )?;
        }

        Ok(())
    }

    fn handle_remove(
        &mut self,
        path: &std::path::Path,
        id: FileId,
        recompile: &mut HashSet<FileId>,
    ) -> anyhow::Result<()> {
        let dependents = self.import_graph.dependents(id);
        self.file_store
            .reset(std::iter::once(id).chain(dependents.iter().copied()));
        self.import_graph.remove(id);

        // TODO: This will probably get easier when we store html state in memory for Stage 3
        if self.config.is_match(path) {
            let output_path = self
                .config
                .output_directory
                .join(id.vpath().get_without_slash())
                .with_extension("html");
            match fs::remove_file(&output_path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    let had_errors = self
                        .diagnostics
                        .get(&id)
                        .is_some_and(|(_, errors)| !errors.is_empty());
                    if !had_errors {
                        return Err(anyhow!(
                            "bug: output missing for successfully compiled {}",
                            path.display()
                        ));
                    }
                }
                Err(e) => {
                    return Err(anyhow::Error::from(e)
                        .context(format!("failed to remove output for {}", path.display())));
                }
            }
            self.outputs.remove(&id);
            self.diagnostics.remove(&id);
        }

        recompile.extend(dependents);
        Ok(())
    }

    pub fn watch(&mut self) -> anyhow::Result<()> {
        if self.config.output_directory.exists() {
            fs::remove_dir_all(&self.config.output_directory)?;
        }
        fs::create_dir(&self.config.output_directory)?;

        // TODO: No reason to make this a vec
        let paths: Vec<_> = WalkDir::new(&self.config.root)
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
            })
            .collect::<Result<_, _>>()?;

        for path in paths {
            let virtual_path = VirtualPath::virtualize(&self.config.root, &path)
                .map_err(|e| anyhow!("failed to virtualize path: {e:?}"))?;
            let id = FileId::new(RootedPath::new(VirtualRoot::Project, virtual_path));
            self.build(id)?;
        }
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
                                    self.handle_remove(path, id, &mut recompile)?;
                                }
                                _ => {}
                            }
                        }
                    }

                    for id in recompile.drain() {
                        self.build(id)?;
                    }
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
