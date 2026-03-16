use std::collections::HashSet;
use std::fs;
use std::io::{IsTerminal, Write};
use std::num::NonZeroU16;
use std::sync::mpsc;
use std::time::Duration;

use anyhow::anyhow;
use notify_debouncer_full::notify::{
    EventKind as NotifyEventKind, RecursiveMode, event::ModifyKind,
};
use notify_debouncer_full::{DebounceEventResult, new_debouncer};
use petgraph::graphmap::DiGraphMap;
use petgraph::visit::{Bfs, Reversed};
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
    world::{DependenciesWorld, Resources, SystemWorld},
};

// TODO: Deduplicate with build.rs (but not yet)
const USER_AGENT: &str = "weibian";

const DEBOUNCE_TIMEOUT: Duration = Duration::from_millis(500);

pub struct Watcher {
    file_store: FileStore<SystemFiles>,
    resources: Resources,
    import_graph: DiGraphMap<NonZeroU16, ()>,
    compiler: Compiler,
    config: BuildConfig,
}

impl Watcher {
    /// Creates a `WatchState` from a build configuration.
    pub fn new(config: BuildConfig) -> Self {
        let downloader = SystemDownloader::new(USER_AGENT);
        let packages = SystemPackages::new(downloader);
        let file_loader = SystemFiles::new(FsRoot::new(config.input_directory.clone()), packages);
        let file_store = FileStore::new(file_loader);

        Self {
            file_store,
            resources: Resources::default(),
            import_graph: DiGraphMap::new(),
            compiler: Compiler::default(),
            config,
        }
    }

    /// Performs the initial build then enters the watch loop, recompiling on
    /// file system changes until the process is terminated.
    ///
    /// Fatal errors (I/O failures, watcher errors, virtualization failures)
    /// are returned as `Err`.
    pub fn watch(&mut self) -> anyhow::Result<()> {
        if self.config.output_directory.exists() {
            fs::remove_dir_all(&self.config.output_directory)?;
        }
        fs::create_dir(&self.config.output_directory)?;

        let ids = WalkDir::new(&self.config.input_directory)
            .into_iter()
            .filter_map(|result| match result {
                Ok(entry) => {
                    if entry.file_type().is_file() && self.config.is_match(entry.path()) {
                        let result =
                            VirtualPath::virtualize(&self.config.input_directory, entry.path())
                                .map(|vpath| {
                                    FileId::new(RootedPath::new(VirtualRoot::Project, vpath))
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
            let world =
                DependenciesWorld::new(SystemWorld::new(id, &self.resources, &self.file_store));

            self.compiler.compile(&world, id);

            let (_, dependencies) = world.into_inner();

            update_dependencies(&mut self.import_graph, id, dependencies);
        }

        self.compiler
            .process(&self.config)?
            .apply(&self.config.output_directory)?;
        self.emit_diagnostics()?;

        let (sender, receiver) = mpsc::channel::<DebounceEventResult>();
        let mut debouncer = new_debouncer(DEBOUNCE_TIMEOUT, None, sender)?;

        debouncer.watch(&self.config.input_directory, RecursiveMode::Recursive)?;

        let mut recompile: HashSet<FileId> = HashSet::new();

        for result in receiver {
            for event in self.parse_events(result)? {
                self.file_store.reset(event.id);

                let start = event.id.into_raw();
                let reversed = Reversed(&self.import_graph);
                let mut bfs = Bfs::new(reversed, start);

                while let Some(raw) = bfs.next(reversed) {
                    if raw != start {
                        let dependency = FileId::from_raw(raw);

                        self.file_store.reset(dependency);
                        if event.is_source {
                            recompile.insert(dependency);
                        }
                    }
                }

                match event.kind {
                    EventKind::Update => {
                        if event.is_source {
                            recompile.insert(event.id);
                        }
                    }
                    EventKind::Remove => {
                        self.import_graph.remove_node(event.id.into_raw());

                        if event.is_source {
                            self.compiler.remove(event.id);
                        }
                    }
                }
            }

            for id in recompile.drain() {
                let world =
                    DependenciesWorld::new(SystemWorld::new(id, &self.resources, &self.file_store));

                self.compiler.compile(&world, id);

                let (_, dependencies) = world.into_inner();

                update_dependencies(&mut self.import_graph, id, dependencies);
            }

            self.compiler
                .process(&self.config)?
                .apply(&self.config.output_directory)?;
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

                let virtual_path = VirtualPath::virtualize(&self.config.input_directory, path)
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

    /// Clears the screen (if stderr is a terminal) and emits all diagnostics.
    fn emit_diagnostics(&self) -> anyhow::Result<()> {
        let mut stderr = StandardStream::stderr(ColorChoice::Auto);

        if std::io::stderr().is_terminal() {
            write!(stderr, "\x1B[2J\x1B[1;1H")?;
            stderr.flush()?;
        }

        for (&id, (warnings, errors)) in self.compiler.compile_diagnostics() {
            let world = SystemWorld::new(id, &self.resources, &self.file_store);
            emit(
                &mut stderr,
                &world,
                warnings.iter().chain(errors.iter()),
                DiagnosticFormat::Human,
            )?;
        }

        for (&id, errors) in self.compiler.process_diagnostics() {
            let world = SystemWorld::new(id, &self.resources, &self.file_store);
            emit(&mut stderr, &world, errors.iter(), DiagnosticFormat::Human)?;
        }

        Ok(())
    }
}

/// Updates the import edges for `id` in `graph` given its new set of
/// dependencies, removing stale edges and adding new ones.
fn update_dependencies(
    graph: &mut DiGraphMap<NonZeroU16, ()>,
    id: FileId,
    dependencies: HashSet<FileId>,
) {
    let raw_id = id.into_raw();
    let old: HashSet<FileId> = graph.neighbors(raw_id).map(FileId::from_raw).collect();
    for &dependency in old.difference(&dependencies) {
        graph.remove_edge(raw_id, dependency.into_raw());
    }
    for &dependency in dependencies.difference(&old) {
        graph.add_edge(raw_id, dependency.into_raw(), ());
    }
}

/// Whether a file was updated (created or modified) or removed.
#[derive(Copy, Clone)]
pub enum EventKind {
    Update,
    Remove,
}

/// A parsed file system event with its [`FileId`] and whether it is a
/// compiled source file (as opposed to a library dependency).
pub struct Event {
    pub id: FileId,
    pub kind: EventKind,
    pub is_source: bool,
}
