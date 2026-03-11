use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Mutex;

use chrono::{Datelike, FixedOffset, Local, Utc};
use typst::diag::FileResult;
use typst::foundations::{Bytes, Datetime, Duration};
use typst::syntax::{FileId, Source, VirtualRoot};
use typst::text::{Font, FontBook};
use typst::utils::LazyHash;
use typst::{Feature, Features, Library, LibraryExt, World};
use typst_kit::diagnostics::DiagnosticWorld;
use typst_kit::downloader::SystemDownloader;
use typst_kit::files::{FileStore, FsRoot, SystemFiles};
use typst_kit::fonts::FontStore;
use typst_kit::packages::SystemPackages;

/// Holds the Typst standard library and a font store.
pub struct Resources {
    pub library: LazyHash<Library>,
    pub fonts: FontStore,
}

impl Resources {
    pub fn new() -> Self {
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
    main: FileId,
    resources: &'a Resources,
    files: &'a FileStore<SystemFiles>,
    now: chrono::DateTime<Utc>,
    dependencies: Mutex<HashSet<FileId>>,
}

impl<'a> SystemWorld<'a> {
    pub fn new(main: FileId, resources: &'a Resources, files: &'a FileStore<SystemFiles>) -> Self {
        Self {
            main,
            resources,
            files,
            now: Utc::now(),
            dependencies: Mutex::new(HashSet::new()),
        }
    }

    /// Consumes the world and returns all `FileId`s accessed during this
    /// compilation, excluding the main file itself.
    pub fn into_dependencies(self) -> HashSet<FileId> {
        self.dependencies.into_inner().unwrap()
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
        if id != self.main {
            self.dependencies.lock().unwrap().insert(id);
        }
        self.files.source(id)
    }

    fn file(&self, id: FileId) -> FileResult<Bytes> {
        if id != self.main {
            self.dependencies.lock().unwrap().insert(id);
        }
        self.files.file(id)
    }

    fn font(&self, index: usize) -> Option<Font> {
        self.resources.fonts.font(index)
    }

    fn today(&self, offset: Option<Duration>) -> Option<Datetime> {
        let now = if offset.is_some() {
            self.now.fixed_offset()
        } else {
            self.now.with_timezone(&Local).fixed_offset()
        };

        let with_offset = match offset {
            None => now,
            Some(offset) => {
                let seconds = offset.seconds().trunc();
                if !seconds.is_finite()
                    || seconds < f64::from(i32::MIN)
                    || seconds > f64::from(i32::MAX)
                {
                    return None;
                }
                now.with_timezone(&FixedOffset::east_opt(seconds as i32)?)
            }
        };

        Datetime::from_ymd(
            with_offset.year(),
            with_offset.month().try_into().ok()?,
            with_offset.day().try_into().ok()?,
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
