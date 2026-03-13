use std::collections::HashSet;
use std::fs;
use std::io::{IsTerminal, Write};
use std::sync::mpsc;
use std::time::Duration;

use anyhow::anyhow;
use notify_debouncer_full::notify::{
    EventKind as NotifyEventKind, RecursiveMode, event::ModifyKind,
};
use notify_debouncer_full::{DebounceEventResult, new_debouncer};
use termcolor::{ColorChoice, StandardStream};
use typst_kit::{
    diagnostics::{DiagnosticFormat, emit},
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

            let world =
                DependenciesWorld::new(SystemWorld::new(id, &self.resources, &self.file_store));

            self.compiler.compile(&world, id)?;

            let (_, dependencies) = world.into_inner();

            self.import_graph.update(id, dependencies);
        }

        self.compiler.process(&self.config.output_directory)?;
        self.emit_diagnostics()?;

        let (sender, receiver) = mpsc::channel::<DebounceEventResult>();
        let mut debouncer = new_debouncer(DEBOUNCE_TIMEOUT, None, sender)?;

        debouncer.watch(&self.config.root, RecursiveMode::Recursive)?;

        let mut recompile: HashSet<FileId> = HashSet::new();

        for result in receiver {
            for event in self.parse_events(result)? {
                let dependents = self.import_graph.dependents(event.id);

                self.file_store
                    .reset(std::iter::once(event.id).chain(dependents.iter().copied()));

                match event.kind {
                    EventKind::Update => {
                        if event.is_source {
                            recompile.insert(event.id);
                        }
                    }
                    EventKind::Remove => {
                        self.import_graph.remove(event.id);
                        if event.is_source {
                            self.compiler.remove(event.id);
                        }
                    }
                }

                recompile.extend(dependents);
            }

            for id in recompile.drain() {
                let world =
                    DependenciesWorld::new(SystemWorld::new(id, &self.resources, &self.file_store));

                self.compiler.compile(&world, id)?;

                let (_, dependencies) = world.into_inner();

                self.import_graph.update(id, dependencies);
            }

            if self.config.output_directory.exists() {
                fs::remove_dir_all(&self.config.output_directory)?;
            }
            fs::create_dir(&self.config.output_directory)?;

            self.compiler.process(&self.config.output_directory)?;
            self.emit_diagnostics()?;

            comemo::evict(10);
        }

        Ok(())
    }

    /// Parses a batch of raw debouncer events into [`Event`]s.
    ///
    /// Filters out noise (access/metadata events), skips paths inside the
    /// output directory, virtualizes paths to [`FileId`]s, and collapses
    /// Create/Modify into [`EventKind::Update`].
    ///
    /// Returns `Err` if the debouncer reported errors or if any path fails
    /// to virtualize.
    fn parse_events(&self, result: DebounceEventResult) -> anyhow::Result<Vec<Event>> {
        // TODO: Maybe handle the vec of errors better
        let raw_events = result.map_err(|errors| {
            anyhow!(
                "watch errors: {}",
                errors
                    .iter()
                    .map(|e| e.to_string())
                    .collect::<Vec<_>>()
                    .join("; ")
            )
        })?;

        let mut events = Vec::new();

        for raw_event in raw_events {
            let kind = match raw_event.kind {
                NotifyEventKind::Create(_) | NotifyEventKind::Modify(_)
                    if !matches!(
                        raw_event.kind,
                        NotifyEventKind::Modify(ModifyKind::Metadata(_))
                            | NotifyEventKind::Modify(ModifyKind::Other)
                    ) =>
                {
                    EventKind::Update
                }
                NotifyEventKind::Remove(_) => EventKind::Remove,
                _ => continue,
            };

            for path in &raw_event.paths {
                if path.starts_with(&self.config.output_directory) {
                    continue;
                }

                let virtual_path = VirtualPath::virtualize(&self.config.root, path)
                    .map_err(|e| anyhow!("failed to virtualize {}: {e:?}", path.display()))?;

                let id = FileId::new(RootedPath::new(VirtualRoot::Project, virtual_path));
                let is_source = self.config.is_match(path);

                events.push(Event {
                    id,
                    kind,
                    is_source,
                });
            }
        }

        Ok(events)
    }

    fn emit_diagnostics(&self) -> anyhow::Result<()> {
        let mut stderr = StandardStream::stderr(ColorChoice::Auto);

        if std::io::stderr().is_terminal() {
            write!(stderr, "\x1B[2J\x1B[1;1H")?;
            stderr.flush()?;
        }

        for (&id, (warnings, errors)) in self.compiler.file_diagnostics() {
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
}

#[derive(Copy, Clone)]
pub enum EventKind {
    Update,
    Remove,
}

pub struct Event {
    pub id: FileId,
    pub kind: EventKind,
    pub is_source: bool,
}
