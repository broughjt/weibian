use std::collections::HashMap;
use std::mem;
use std::str;
use std::str::Utf8Error;
use std::sync::Mutex;

use typst::diag::FileResult;
use typst::foundations::Bytes;
use typst::syntax::{FileId, Source};
use typst_kit::files::FileLoader;

/// Loads files and sources and caches them as necessary.
///
/// Adapted from `typst_kit::files::FileStore`. The key difference is that
/// [`reset`](FileStore::reset) accepts individual [`FileId`]s to invalidate
/// rather than resetting all slots at once, enabling selective cache
/// invalidation for watch-mode recompilation.
pub struct FileStore<L> {
    loader: L,
    slots: Mutex<HashMap<FileId, FileSlot>>,
}

impl<L> FileStore<L>
where
    L: FileLoader,
{
    pub fn new(loader: L) -> Self {
        Self {
            loader,
            slots: Mutex::new(HashMap::new()),
        }
    }

    pub fn loader(&self) -> &L {
        &self.loader
    }

    /// Retrieves the given file as a Typst source.
    pub fn source(&self, id: FileId) -> FileResult<Source> {
        self.slot(id, |slot| slot.source(&self.loader, id))
    }

    /// Retrieves the given file as raw bytes.
    pub fn file(&self, id: FileId) -> FileResult<Bytes> {
        self.slot(id, |slot| slot.file(&self.loader, id))
    }

    /// Resets the given file slots.
    ///
    /// On subsequent accesses, invalidated files are re-loaded through the
    /// underlying loader. When a previously parsed source slot is reset, the
    /// stale source is retained so it can be updated in-place via
    /// [`Source::replace`], giving better incremental compilation performance
    /// than creating a new source from scratch.
    pub fn reset(&mut self, ids: impl IntoIterator<Item = FileId>) {
        let slots = self.slots.get_mut().unwrap();
        for id in ids {
            if let Some(slot) = slots.get_mut(&id) {
                slot.reset();
            }
        }
    }

    fn slot<F, T>(&self, id: FileId, f: F) -> FileResult<T>
    where
        F: FnOnce(&mut FileSlot) -> FileResult<T>,
    {
        let mut map = self.slots.lock().unwrap();
        f(map.entry(id).or_default())
    }
}

/// Holds the state for a single cached file.
enum FileSlot {
    /// Nothing loaded, but may hold a stale source from before a reset that
    /// can be updated in-place rather than re-parsed from scratch.
    ///
    /// Transitions to `Loaded` or `Parsed` on next access.
    Empty(Stale<Source>),
    /// Loaded as raw bytes (via `file()`), not yet parsed as a source.
    /// May still hold a stale source for later reuse.
    ///
    /// Transitions to `Parsed` when a source is requested.
    Loaded(FileResult<Bytes>, Stale<Source>),
    /// Loaded and parsed as a source (via `source()`).
    ///
    /// Where possible the bytes are backed by the source string (via
    /// `Bytes::from_string`) so that `file()` and `source()` share the same
    /// allocation. This is not possible when the data has a UTF-8 BOM, since
    /// the BOM is stripped for the source but retained in the raw bytes.
    Parsed(Result<Source, Utf8Error>, Bytes),
}

/// A stale value from a previous compilation that may be reused.
type Stale<T> = Option<T>;

impl FileSlot {
    fn reset(&mut self) {
        let stale = match mem::take(self) {
            Self::Parsed(Ok(source), _) => Some(source),
            _ => None,
        };
        *self = Self::Empty(stale);
    }

    fn file(&mut self, loader: &impl FileLoader, id: FileId) -> FileResult<Bytes> {
        match self {
            Self::Empty(stale) => {
                let result = loader.load(id);
                *self = Self::Loaded(result.clone(), mem::take(stale));
                result
            }
            Self::Loaded(result, _) => result.clone(),
            Self::Parsed(_, bytes) => Ok(bytes.clone()),
        }
    }

    fn source(&mut self, loader: &impl FileLoader, id: FileId) -> FileResult<Source> {
        let (bytes, stale) = match self {
            Self::Empty(stale) => match loader.load(id) {
                Ok(bytes) => (bytes, mem::take(stale)),
                Err(err) => {
                    *self = Self::Loaded(Err(err.clone()), mem::take(stale));
                    return Err(err);
                }
            },
            Self::Loaded(Ok(_), _) => match mem::take(self) {
                Self::Loaded(Ok(bytes), stale) => (bytes, stale),
                _ => unreachable!(),
            },
            Self::Loaded(Err(err), _) => return Err(err.clone()),
            Self::Parsed(source, _) => return Ok(source.clone()?),
        };

        const UTF8_BOM: &[u8] = b"\xef\xbb\xbf";
        let without_bom = bytes.strip_prefix(UTF8_BOM);

        let (result, bytes) = if let Some(mut source) = stale {
            let result = str::from_utf8(without_bom.unwrap_or(&bytes)).map(|new| {
                source.replace(new);
                source
            });
            (result, bytes)
        } else if let Some(rest) = without_bom {
            (
                str::from_utf8(rest).map(|text| Source::new(id, text.into())),
                bytes,
            )
        } else {
            match bytes.into_string().map(|text| Source::new(id, text)) {
                Ok(source) => (Ok(source.clone()), Bytes::from_string(source)),
                Err(err) => (Err(err.error), err.bytes),
            }
        };

        *self = Self::Parsed(result.clone(), bytes);
        Ok(result?)
    }
}

impl Default for FileSlot {
    fn default() -> Self {
        Self::Empty(None)
    }
}
