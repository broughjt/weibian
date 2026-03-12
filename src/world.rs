use std::collections::HashSet;
use std::sync::Mutex;

use jiff::tz::{Offset, TimeZone};
use jiff::Timestamp;
use typst::diag::FileResult;
use typst::foundations::{Bytes, Datetime, Duration};
use typst::syntax::{FileId, Source, VirtualRoot};
use typst::text::{Font, FontBook};
use typst::utils::LazyHash;
use typst::{Feature, Features, Library, LibraryExt, World};
use typst_kit::diagnostics::DiagnosticWorld;
use typst_kit::files::SystemFiles;
use typst_kit::fonts::FontStore;

use crate::file_store::FileStore;

/// Holds the Typst standard library and a font store.
pub struct Resources {
    pub library: LazyHash<Library>,
    pub fonts: FontStore,
}

impl Default for Resources {
    fn default() -> Self {
        let mut fonts = FontStore::new();
        fonts.extend(typst_kit::fonts::embedded());
        fonts.extend(typst_kit::fonts::system());

        let library = Library::builder()
            .with_features(Features::from_iter([Feature::Html]))
            .build();

        Self {
            library: LazyHash::new(library),
            fonts,
        }
    }
}

/// Ephemeral Typst [`World`] created fresh for each source file compilation.
///
/// Borrows persistent state (`Resources`, `FileStore`) and accumulates the set
/// of `FileId`s accessed during compilation.
pub struct SystemWorld<'a> {
    file_store: &'a FileStore<SystemFiles>,
    resources: &'a Resources,
    main: FileId,
    now: Timestamp,
}

impl<'a> SystemWorld<'a> {
    /// Construct a new [`SystemWorld`].
    pub fn new(
        main: FileId,
        resources: &'a Resources,
        file_store: &'a FileStore<SystemFiles>,
    ) -> Self {
        Self {
            main,
            resources,
            file_store,
            now: Timestamp::now(),
        }
    }
}

impl World for SystemWorld<'_> {
    fn library(&self) -> &LazyHash<Library> {
        &self.resources.library
    }

    fn book(&self) -> &LazyHash<FontBook> {
        self.resources.fonts.book()
    }

    fn main(&self) -> FileId {
        self.main
    }

    fn source(&self, id: FileId) -> FileResult<Source> {
        self.file_store.source(id)
    }

    fn file(&self, id: FileId) -> FileResult<Bytes> {
        self.file_store.file(id)
    }

    fn font(&self, index: usize) -> Option<Font> {
        self.resources.fonts.font(index)
    }

    fn today(&self, offset: Option<Duration>) -> Option<Datetime> {
        let zoned = match offset {
            None => self.now.to_zoned(TimeZone::system()),
            Some(offset) => {
                let seconds = offset.seconds().trunc();

                if !seconds.is_finite()
                    || seconds < f64::from(i32::MIN)
                    || f64::from(i32::MAX) < seconds
                {
                    return None;
                }

                self.now
                    .to_zoned(Offset::from_seconds(seconds as i32).ok()?.to_time_zone())
            }
        };

        Datetime::from_ymd(
            zoned.year().into(),
            zoned.month().try_into().ok()?,
            zoned.day().try_into().ok()?,
        )
    }
}

impl DiagnosticWorld for SystemWorld<'_> {
    fn name(&self, id: FileId) -> String {
        match id.root() {
            VirtualRoot::Project => id.vpath().get_without_slash().into(),
            VirtualRoot::Package(spec) => format!("{spec}{}", id.vpath().get_with_slash()),
        }
    }
}

pub struct DependenciesWorld<W> {
    world: W,
    dependencies: Mutex<HashSet<FileId>>,
}

impl<W> DependenciesWorld<W> {
    /// Construct a new [`DependenciesWorld`].
    pub fn new(world: W) -> Self {
        Self {
            world,
            dependencies: Mutex::new(HashSet::new()),
        }
    }

    /// Consume the world and return all `FileId`s accessed during this
    /// compilation, excluding the main file itself.
    pub fn into_inner(self) -> (W, HashSet<FileId>) {
        (self.world, self.dependencies.into_inner().unwrap())
    }
}

impl<W: World> World for DependenciesWorld<W> {
    fn library(&self) -> &LazyHash<Library> {
        self.world.library()
    }

    fn book(&self) -> &LazyHash<FontBook> {
        self.world.book()
    }

    fn main(&self) -> FileId {
        self.world.main()
    }

    fn source(&self, id: FileId) -> FileResult<Source> {
        if id != self.world.main() {
            self.dependencies.lock().unwrap().insert(id);
        }
        self.world.source(id)
    }

    fn file(&self, id: FileId) -> FileResult<Bytes> {
        if id != self.world.main() {
            self.dependencies.lock().unwrap().insert(id);
        }
        self.world.file(id)
    }

    fn font(&self, index: usize) -> Option<Font> {
        self.world.font(index)
    }

    fn today(&self, offset: Option<Duration>) -> Option<Datetime> {
        self.world.today(offset)
    }
}
