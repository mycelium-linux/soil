//! Host implementation of `soil:vfs` and `soil:index` over `fungos-vfs`.
//!
//! The host owns canonical paths, the VFS namespace, hydration and the derived
//! index. A Spore sees only the capability contracts.

mod embeddings;
mod indexer;

use std::{
    fs::File,
    io::Write,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use anyhow::Context;
use fungos_vfs::{Expect, Hydration, Vfs, VfsError, normalize_path};
use wasmtime::component::{HasSelf, Linker, Resource};

wasmtime::component::bindgen!({
    path: "wit/vfs",
    world: "host",
    with: {
        "soil:vfs-mut/vfs-mut.writer": VfsWriter,
    },
});

pub use embeddings::EmbeddingProvider;
#[cfg(feature = "model2vec")]
pub use embeddings::Model2VecEmbedder;
#[cfg(feature = "remote")]
pub use embeddings::RemoteEmbeddingProvider;
/// The role a root plays. Re-exported because [`VfsHost::open_with_role`] takes
/// one: a caller cannot name a parameter type it cannot reach.
pub use fungos_vfs::NodeRole;
use indexer::{IndexBackend, Node, NodeKind, SqliteIndex};
pub use self::soil::index::index;
pub use self::soil::vfs::vfs;
pub use self::soil::vfs_cid::cid;
pub use self::soil::vfs_mut::vfs_mut;

use self::soil::vfs_mut::vfs_mut as vfs_mut_api;

pub(crate) const MAX_PAGE: u32 = 512;
const MAX_RANGE: u32 = 1024 * 1024;
const MAX_MUTATION_BYTES: usize = 16 * 1024 * 1024;
const MAX_STREAM_BYTES: u64 = 512 * 1024 * 1024;

static NEXT_WRITER: AtomicU64 = AtomicU64::new(0);

/// Optional content transport used when a VFS path is only a placeholder.
pub trait ContentHydrator: Send + Sync {
    fn fetch(&self, path: &str, content_id: &str, size: u64) -> Result<Vec<u8>, VfsError>;
}

struct DynHydrator(Arc<dyn ContentHydrator>);

impl fungos_vfs::Hydrator for DynHydrator {
    fn fetch(
        &self,
        path: &str,
        content_id: &fungos_vfs::ContentId,
        size: u64,
    ) -> Result<Vec<u8>, VfsError> {
        self.0.fetch(path, &content_id.to_string(), size)
    }
}

/// What a file is when the caller did not say: readable, not executable.
/// Mirrors the default `fungos-vfs` applies to a path nobody has written.
pub const NEW_FILE_MODE: u32 = 0o644;

#[derive(Clone)]
pub struct VfsHost {
    vfs: Arc<Mutex<Vfs>>,
    hydrator: Option<Arc<dyn ContentHydrator>>,
    index: Arc<Mutex<Box<dyn IndexBackend>>>,
    embeddings: Option<Arc<dyn EmbeddingProvider>>,
    staging: PathBuf,
}

/// A host-owned temp object for one streamed VFS write.
///
/// The guest never sees the host path. Bytes go to disk as they arrive; the
/// namespace and content store are changed only on `finish`.
pub struct VfsWriter {
    vfs: Arc<Mutex<Vfs>>,
    index: Arc<Mutex<Box<dyn IndexBackend>>>,
    path: String,
    media_type: String,
    expected_version: Option<u64>,
    mode: Option<u32>,
    file: Option<File>,
    temp: PathBuf,
    bytes: u64,
    max_bytes: u64,
    finished: bool,
}

impl VfsHost {
    pub fn open(root: impl AsRef<std::path::Path>) -> anyhow::Result<Self> {
        Self::open_with_role(root, NodeRole::Full)
    }

    /// Open with the default derived index at `<root>/.fungos/soil-index.sqlite3`.
    pub fn open_with_role(
        root: impl AsRef<std::path::Path>,
        role: NodeRole,
    ) -> anyhow::Result<Self> {
        let root = root.as_ref();
        let index_path = root.join(".fungos/soil-index.sqlite3");
        Self::open_with_role_and_index(root, role, index_path)
    }

    /// Open against an explicit content store.
    ///
    /// `None` means the machine-wide store every root on this host shares. A
    /// test, a service or a sandbox passes its own: content is the part of the
    /// VFS that is *not* per-root, so two processes sharing one store can see
    /// each other's writes, and a test that leaks into it is a test that can
    /// fail only when it runs next to another one.
    pub fn open_with_role_and_store(
        root: impl AsRef<std::path::Path>,
        role: NodeRole,
        store: Option<&std::path::Path>,
    ) -> anyhow::Result<Self> {
        let root = root.as_ref();
        let index_path = root.join(".fungos/soil-index.sqlite3");
        Self::open_with_store(root, role, index_path, store)
    }

    /// Open with an explicit derived-index database. The database is a cache and
    /// can be deleted at any time; it must not live inside `records`.
    pub fn open_with_index(
        root: impl AsRef<std::path::Path>,
        index_path: impl AsRef<std::path::Path>,
    ) -> anyhow::Result<Self> {
        Self::open_with_role_and_index(root, NodeRole::Full, index_path)
    }

    pub fn open_with_role_and_index(
        root: impl AsRef<std::path::Path>,
        role: NodeRole,
        index_path: impl AsRef<std::path::Path>,
    ) -> anyhow::Result<Self> {
        Self::open_with_store(root, role, index_path, None)
    }

    /// The full form: root, role, index location and content store.
    pub fn open_with_store(
        root: impl AsRef<std::path::Path>,
        role: NodeRole,
        index_path: impl AsRef<std::path::Path>,
        store: Option<&std::path::Path>,
    ) -> anyhow::Result<Self> {
        let root = root.as_ref();
        let mut vfs = Vfs::open_with(root, role, store).context("open fungos-vfs")?;
        let mut index = SqliteIndex::open(index_path).context("open soil index")?;
        index.refresh(&mut vfs).context("refresh soil index")?;
        let staging = root.join(".fungos/staging");
        std::fs::create_dir_all(&staging).context("create VFS staging")?;
        Ok(Self {
            vfs: Arc::new(Mutex::new(vfs)),
            hydrator: None,
            index: Arc::new(Mutex::new(Box::new(index))),
            embeddings: None,
            staging,
        })
    }

    pub fn with_hydrator(mut self, hydrator: Arc<dyn ContentHydrator>) -> Self {
        self.hydrator = Some(hydrator);
        self
    }

    /// Open a streaming writer. The path is validated now, so a caller learns
    /// about a bad workspace path before uploading a large object.
    pub fn begin_writer(
        &self,
        path: String,
        media_type: String,
        expected_size: Option<u64>,
        expected_version: Option<u64>,
        mode: Option<u32>,
    ) -> Result<VfsWriter, vfs_mut_api::Error> {
        let path = normalize_path(&path).map_err(|_| vfs_mut_api::Error::Invalid)?;
        if media_type.trim().is_empty() {
            return Err(vfs_mut_api::Error::Invalid);
        }
        let max_bytes = expected_size
            .unwrap_or(MAX_STREAM_BYTES)
            .min(MAX_STREAM_BYTES);
        let id = NEXT_WRITER.fetch_add(1, Ordering::Relaxed);
        let temp = self.staging.join(format!("write-{}-{id}", std::process::id()));
        let file = File::create(&temp).map_err(|_| vfs_mut_api::Error::Io)?;
        Ok(VfsWriter {
            vfs: self.vfs.clone(),
            index: self.index.clone(),
            path,
            media_type,
            expected_version,
            mode,
            file: Some(file),
            temp,
            bytes: 0,
            max_bytes,
            finished: false,
        })
    }

    /// Opt in to semantic indexing with an explicit host-selected provider.
    pub fn with_embeddings(
        mut self,
        embeddings: Arc<dyn EmbeddingProvider>,
    ) -> anyhow::Result<Self> {
        self.embeddings = Some(embeddings);
        self.refresh_embeddings()?;
        Ok(self)
    }

    pub fn refresh_index(&self) -> anyhow::Result<index::IndexState> {
        self.refresh_view()?;
        self.refresh_embeddings()?;
        Ok(self.index_read().state())
    }

    /// Refresh the namespace cache without paying for semantic embeddings.
    /// A lookup only needs the current files and directories.
    fn refresh_view(&self) -> anyhow::Result<()> {
        let mut vfs = self.vfs.lock().expect("vfs poisoned");
        let mut index = self.index.lock().expect("index poisoned");
        index.refresh(&mut vfs)
    }

    fn refresh_embeddings(&self) -> anyhow::Result<()> {
        let Some(provider) = &self.embeddings else {
            return Ok(());
        };
        let model = provider.model_id();
        loop {
            let chunks = {
                let index = self.index_read();
                index.missing_chunks(model, 64)?
            };
            if chunks.is_empty() {
                break;
            }
            let texts: Vec<_> = chunks.iter().map(|(_, text)| text.clone()).collect();
            let vectors = provider.embed(&texts)?;
            if vectors.len() != chunks.len() {
                anyhow::bail!(
                    "embedding provider returned {} vectors for {} chunks",
                    vectors.len(),
                    chunks.len()
                );
            }
            let mut index = self.index.lock().expect("index poisoned");
            for ((chunk_id, _), vector) in chunks.iter().zip(vectors) {
                if vector.len() != provider.dimension() {
                    anyhow::bail!(
                        "embedding dimension {} does not match model {}",
                        vector.len(),
                        provider.dimension()
                    );
                }
                index.put_vector(chunk_id, model, &vector)?;
            }
        }
        Ok(())
    }

    fn knn_search(
        &self,
        text: &str,
        parent: Option<&str>,
        media_type_prefix: Option<&str>,
        offset: u64,
        limit: u32,
    ) -> Result<index::Page, index::Error> {
        let Some(provider) = &self.embeddings else {
            return Err(index::Error::Unsupported);
        };
        let vector = provider
            .embed(&[text.to_string()])
            .map_err(index_error)?
            .into_iter()
            .next()
            .ok_or(index::Error::Io)?;
        let parent = parent
            .map(normalize_parent)
            .transpose()
            .map_err(|_| index::Error::Invalid)?;
        let index = self.index_read();
        let wanted = usize::try_from(offset.saturating_add(limit as u64))
            .unwrap_or(MAX_PAGE as usize)
            .saturating_mul(2)
            .max(MAX_PAGE as usize);
        let raw = index
            .semantic_search(provider.model_id(), &vector, wanted)
            .map_err(index_error)?;
        let mut hits = Vec::new();
        for (path, score) in raw {
            let Some(node) = index.entry(&path) else {
                continue;
            };
            if let Some(parent) = &parent {
                let start = if parent.is_empty() {
                    0
                } else {
                    parent.len() + 1
                };
                let rest = path.get(start..).unwrap_or_default();
                let direct = path.starts_with(parent) && !rest.contains('/');
                if !direct {
                    continue;
                }
            }
            if let Some(prefix) = media_type_prefix
                && !node.entry.media_type.starts_with(prefix)
            {
                continue;
            }
            hits.push(index::Hit {
                path,
                score: (score.max(0.0) * 1_000_000.0) as u32,
                snippet: Some(node.text.chars().take(160).collect()),
                matched_by: Some(index::MatchSource::Embedding),
            });
        }
        hits.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.path.cmp(&b.path)));
        let start = usize::try_from(offset).unwrap_or(usize::MAX);
        let total = hits.len() as u64;
        let page: Vec<_> = hits.into_iter().skip(start).take(limit as usize).collect();
        let consumed = offset.saturating_add(page.len() as u64);
        Ok(index::Page {
            hits: page,
            next: (consumed < total).then_some(consumed),
            generation: index.state().generation,
        })
    }

    fn index_read(&self) -> std::sync::MutexGuard<'_, Box<dyn IndexBackend>> {
        self.index.lock().expect("index poisoned")
    }

    fn read_path(&self, path: &str, offset: u64, length: u32) -> Result<(Vec<u8>, u64), vfs::Error> {
        if length == 0 || length > MAX_RANGE {
            return Err(vfs::Error::OverBudget);
        }
        let mut vfs = self.vfs.lock().expect("vfs poisoned");
        let mut bytes = match vfs.get(path) {
            Ok(bytes) => bytes,
            Err(VfsError::NotHydrated(_)) => {
                let Some(hydrator) = &self.hydrator else {
                    return Err(vfs::Error::NotHydrated);
                };
                let wrapped = DynHydrator(hydrator.clone());
                vfs.get_or_hydrate(path, &wrapped).map_err(vfs_error)?
            }
            Err(error) => return Err(vfs_error(error)),
        };
        let start = usize::try_from(offset).unwrap_or(usize::MAX);
        if start > bytes.len() {
            return Err(vfs::Error::Invalid);
        }
        let total = bytes.len() as u64;
        let end = start.saturating_add(length as usize).min(bytes.len());
        Ok((bytes.drain(start..end).collect(), total))
    }
}

fn vfs_error(error: VfsError) -> vfs::Error {
    match error {
        VfsError::NotFound(_) => vfs::Error::NotFound,
        VfsError::NotHydrated(_) => vfs::Error::NotHydrated,
        VfsError::InvalidPath(_) => vfs::Error::Invalid,
        VfsError::ReadOnly => vfs::Error::Denied,
        VfsError::Io(_) | VfsError::Sql(_) | VfsError::Transport(_) => vfs::Error::Io,
        VfsError::InvalidContentId(_)
        | VfsError::Conflict { .. }
        | VfsError::Fenced { .. }
        | VfsError::SnapshotRefused { .. }
        | VfsError::AlreadyExists(_)
        | VfsError::NotEmpty(_)
        | VfsError::Leased { .. } => vfs::Error::Io,
        VfsError::RoleMismatch { .. } => vfs::Error::Denied,
        VfsError::Config(_)
        | VfsError::NoDataDirectory
        | VfsError::CorruptMeta { .. }
        | VfsError::GenerationExhausted => vfs::Error::Io,
    }
}

fn index_error(error: anyhow::Error) -> index::Error {
    eprintln!("soil index: {error:#}");
    index::Error::Io
}

impl VfsWriter {
    pub fn write(&mut self, data: &[u8]) -> Result<u64, vfs_mut_api::Error> {
        let next = self
            .bytes
            .checked_add(data.len() as u64)
            .ok_or(vfs_mut_api::Error::OverBudget)?;
        if next > self.max_bytes {
            return Err(vfs_mut_api::Error::OverBudget);
        }
        let file = self.file.as_mut().ok_or(vfs_mut_api::Error::Io)?;
        file.write_all(data).map_err(|_| vfs_mut_api::Error::Io)?;
        self.bytes = next;
        Ok(self.bytes)
    }

    pub fn finish(&mut self) -> Result<vfs_mut_api::WriteResult, vfs_mut_api::Error> {
        if self.finished {
            return Err(vfs_mut_api::Error::Invalid);
        }
        if let Some(mut file) = self.file.take() {
            file.flush().map_err(|_| vfs_mut_api::Error::Io)?;
        }
        let data = std::fs::read(&self.temp).map_err(|_| vfs_mut_api::Error::Io)?;
        let entry = {
            let mut vfs = self.vfs.lock().expect("vfs poisoned");
            let expect = self
                .expected_version
                .map(Expect::Version)
                .unwrap_or(Expect::Any);
            let result = match self.mode {
                Some(mode) => vfs.put_with_mode(&self.path, &data, &self.media_type, mode, expect),
                None => vfs.put(&self.path, &data, &self.media_type, expect),
            };
            result.map_err(mutation_error)?
        };
        self.index
            .lock()
            .expect("index poisoned")
            .refresh(&mut self.vfs.lock().expect("vfs poisoned"))
            .map_err(|_| vfs_mut_api::Error::Io)?;
        let _ = std::fs::remove_file(&self.temp);
        self.finished = true;
        Ok(write_result(entry))
    }

    pub fn abort(&mut self) -> Result<(), vfs_mut_api::Error> {
        self.file.take();
        let _ = std::fs::remove_file(&self.temp);
        self.finished = true;
        Ok(())
    }
}

impl Drop for VfsWriter {
    fn drop(&mut self) {
        self.file.take();
        let _ = std::fs::remove_file(&self.temp);
    }
}

impl vfs::Host for VfsHost {
    fn ls(&mut self, parent: String, offset: u64, limit: u32) -> Result<vfs::Page, vfs::Error> {
        if limit == 0 || limit > MAX_PAGE {
            return Err(vfs::Error::OverBudget);
        }
        let parent = normalize_parent(&parent)?;
        // The index is a cache shared by every process on this root. A
        // directory listing must observe writes committed by another host
        // (Scribe's ingest) without requiring a restart.
        self.refresh_view().map_err(|_| vfs::Error::Io)?;
        let index = self.index_read();
        let Some(node) = index.entry(&parent) else {
            return Err(vfs::Error::NotFound);
        };
        if node.kind != NodeKind::Directory {
            return Err(vfs::Error::NotDirectory);
        }
        let children = index.children(&parent);
        let start = usize::try_from(offset).unwrap_or(usize::MAX);
        let entries: Vec<_> = children
            .iter()
            .filter_map(|path| index.entry(path))
            .skip(start)
            .take(limit as usize)
            .map(to_entry)
            .collect();
        let next = if offset.saturating_add(entries.len() as u64) < node.child_count {
            Some(offset + entries.len() as u64)
        } else {
            None
        };
        Ok(vfs::Page { entries, next })
    }

    fn stat(&mut self, path: String) -> Result<vfs::Entry, vfs::Error> {
        let path = normalize_parent(&path)?;
        self.refresh_view().map_err(|_| vfs::Error::Io)?;
        let index = self.index_read();
        let node = index.entry(&path).ok_or(vfs::Error::NotFound)?;
        Ok(to_entry(node))
    }

    fn read(&mut self, path: String, offset: u64, length: u32) -> Result<vfs::Chunk, vfs::Error> {
        let path = normalize_parent(&path)?;
        // A read is the hot path for every capture bundle. The VFS is the
        // authority for bytes; the derived index is not needed to serve them.
        // Refreshing it here listed and rehashed the whole namespace per file.
        let (data, size) = self.read_path(&path, offset, length)?;
        let content_id = self
            .index_read()
            .entry(&path)
            .map(|node| node.entry.content_id.to_string())
            .unwrap_or_default();
        Ok(vfs::Chunk {
            offset,
            eof: offset.saturating_add(data.len() as u64) >= size,
            data,
            content_id,
        })
    }
}

impl cid::Host for VfsHost {
    fn find(
        &mut self,
        content_id: String,
        offset: u64,
        limit: u32,
    ) -> Result<cid::Page, cid::Error> {
        if limit == 0 || limit > MAX_PAGE {
            return Err(cid::Error::OverBudget);
        }
        if !is_content_id(&content_id) {
            return Err(cid::Error::Invalid);
        }
        // The index is derived and may be stale for a path nobody has touched,
        // so it is refreshed the same way `ls` and `stat` refresh it. A caller
        // asking by identity gets the same freshness as one asking by name.
        self.refresh_index().map_err(|_| cid::Error::Io)?;
        let index = self.index_read();
        let hits = index.by_content(&content_id);
        let start = usize::try_from(offset)
            .unwrap_or(usize::MAX)
            .min(hits.len());
        let entries: Vec<vfs::Entry> = hits[start..]
            .iter()
            .take(limit as usize)
            .map(|node| to_entry(node))
            .collect();
        let taken = entries.len();
        let next = if start + taken < hits.len() {
            Some((start + taken) as u64)
        } else {
            None
        };
        Ok(cid::Page { entries, next })
    }
}

/// A content id as the host spells it: `blake3:` and 64 hex digits.
///
/// Checked here rather than in the guest, because the index is keyed by the
/// same string: a malformed id would be a query that can only ever miss, and a
/// caller would read that miss as "the bytes are gone".
fn is_content_id(value: &str) -> bool {
    let Some(hex) = value.strip_prefix("blake3:") else {
        return false;
    };
    hex.len() == 64 && hex.chars().all(|c| c.is_ascii_hexdigit())
}

fn normalize_parent(value: &str) -> Result<String, vfs::Error> {
    if value.is_empty() {
        return Ok(String::new());
    }
    normalize_path(value).map_err(|_| vfs::Error::Invalid)
}

fn to_entry(node: &Node) -> vfs::Entry {
    vfs::Entry {
        path: node.entry.path.clone(),
        name: node
            .entry
            .path
            .rsplit('/')
            .next()
            .unwrap_or(&node.entry.path)
            .to_string(),
        kind: if node.kind == NodeKind::Directory {
            vfs::Kind::Directory
        } else {
            vfs::Kind::File
        },
        size: node.entry.size,
        content_id: node.entry.content_id.to_string(),
        media_type: node.entry.media_type.clone(),
        version: node.entry.version,
        hydration: if node.entry.hydration == Hydration::Placeholder {
            vfs::Hydration::Placeholder
        } else {
            vfs::Hydration::Present
        },
        pinned: node.entry.pinned,
        updated_at_ms: node.entry.updated_at_ms.max(0) as u64,
        mode: node.entry.mode,
        child_count: node.child_count,
    }
}

impl index::Host for VfsHost {
    fn browse(
        &mut self,
        parent: String,
        order: index::Order,
        offset: u64,
        limit: u32,
    ) -> Result<index::Page, index::Error> {
        if limit == 0 || limit > MAX_PAGE {
            return Err(index::Error::OverBudget);
        }
        let parent = normalize_parent(&parent).map_err(|_| index::Error::Invalid)?;
        self.index_read().browse(&parent, order, offset, limit)
    }

    fn search(&mut self, query: index::Query) -> Result<index::Page, index::Error> {
        if query.mode == Some(index::Mode::Semantic) {
            return self.knn_search(
                &query.text,
                query.parent.as_deref(),
                query.media_type_prefix.as_deref(),
                query.offset,
                query.limit,
            );
        }
        self.index_read().search(&query)
    }

    fn knn(&mut self, query: index::KnnQuery) -> Result<index::Page, index::Error> {
        if query.limit == 0 || query.limit > MAX_PAGE {
            return Err(index::Error::OverBudget);
        }
        if query.text.trim().is_empty() {
            return Err(index::Error::Invalid);
        }
        self.knn_search(
            &query.text,
            query.parent.as_deref(),
            query.media_type_prefix.as_deref(),
            query.offset,
            query.limit,
        )
    }

    fn state(&mut self) -> Result<index::IndexState, index::Error> {
        Ok(self.index_read().state())
    }

    fn refresh(&mut self) -> Result<index::IndexState, index::Error> {
        self.refresh_index().map_err(index_error)
    }
}

impl VfsHost {
    fn refresh_after_mutation(&self) -> Result<(), vfs_mut_api::Error> {
        self.refresh_index().map_err(|_| vfs_mut_api::Error::Io)?;
        Ok(())
    }
}

impl vfs_mut_api::Host for VfsHost {
    fn begin_write(
        &mut self,
        _path: String,
        _media_type: String,
        _expected_size: Option<u64>,
        _expected_version: Option<u64>,
        _mode: Option<u32>,
    ) -> Result<Resource<VfsWriter>, vfs_mut_api::Error> {
        Err(vfs_mut_api::Error::Denied)
    }

    fn write(
        &mut self,
        path: String,
        data: Vec<u8>,
        media_type: String,
        expected_version: Option<u64>,
        mode: Option<u32>,
    ) -> Result<vfs_mut_api::WriteResult, vfs_mut_api::Error> {
        if data.len() > MAX_MUTATION_BYTES || media_type.trim().is_empty() {
            return Err(vfs_mut_api::Error::OverBudget);
        }
        // The VFS distinguishes "overwrite" from "replace this version" and
        // "replace a head holding this content"; the ABI's Option<u64> only
        // ever expressed the first two.
        let entry = {
            let mut vfs = self.vfs.lock().expect("vfs poisoned");
            let expect = expected_version
                .map(Expect::Version)
                .unwrap_or(Expect::Any);
            match mode {
                Some(mode) => vfs.put_with_mode(&path, &data, &media_type, mode, expect),
                None => vfs.put(&path, &data, &media_type, expect),
            }
            .map_err(mutation_error)?
        };
        self.refresh_after_mutation()?;
        Ok(write_result(entry))
    }

    fn chmod(
        &mut self,
        path: String,
        mode: u32,
    ) -> Result<vfs_mut_api::WriteResult, vfs_mut_api::Error> {
        let entry = {
            self.vfs
                .lock()
                .expect("vfs poisoned")
                .set_mode(&path, mode)
                .map_err(mutation_error)?
        };
        self.refresh_after_mutation()?;
        Ok(write_result(entry))
    }

    fn remove(&mut self, path: String) -> Result<(), vfs_mut_api::Error> {
        {
            let mut vfs = self.vfs.lock().expect("vfs poisoned");
            if vfs.file(&path).map_err(mutation_error)?.is_none() {
                return Err(vfs_mut_api::Error::NotFound);
            }
            vfs.remove(&path).map_err(mutation_error)?;
        }
        self.refresh_after_mutation()
    }

    fn mkdir(&mut self, path: String) -> Result<(), vfs_mut_api::Error> {
        {
            self.vfs
                .lock()
                .expect("vfs poisoned")
                .mkdir(&path)
                .map_err(mutation_error)?;
        }
        self.refresh_after_mutation()
    }

    fn rmdir(&mut self, path: String) -> Result<(), vfs_mut_api::Error> {
        {
            self.vfs
                .lock()
                .expect("vfs poisoned")
                .rmdir(&path)
                .map_err(mutation_error)?;
        }
        self.refresh_after_mutation()
    }

    fn rename(
        &mut self,
        source: String,
        target: String,
    ) -> Result<vfs_mut_api::WriteResult, vfs_mut_api::Error> {
        let entry = {
            self.vfs
                .lock()
                .expect("vfs poisoned")
                .rename(&source, &target)
                .map_err(mutation_error)?
        };
        self.refresh_after_mutation()?;
        Ok(write_result(entry))
    }
}

impl vfs_mut_api::HostWriter for VfsHost {
    fn write(
        &mut self,
        _writer: Resource<VfsWriter>,
        _data: Vec<u8>,
    ) -> Result<u64, vfs_mut_api::Error> {
        Err(vfs_mut_api::Error::Denied)
    }

    fn finish(
        &mut self,
        _writer: Resource<VfsWriter>,
    ) -> Result<vfs_mut_api::WriteResult, vfs_mut_api::Error> {
        Err(vfs_mut_api::Error::Denied)
    }

    fn abort(&mut self, _writer: Resource<VfsWriter>) -> Result<(), vfs_mut_api::Error> {
        Err(vfs_mut_api::Error::Denied)
    }

    fn drop(&mut self, _writer: Resource<VfsWriter>) -> Result<(), wasmtime::Error> {
        Ok(())
    }
}

fn write_result(entry: fungos_vfs::FileEntry) -> vfs_mut_api::WriteResult {
    vfs_mut_api::WriteResult {
        path: entry.path,
        version: entry.version,
        content_id: entry.content_id.to_string(),
        size: entry.size,
        mode: entry.mode,
    }
}

fn mutation_error(error: VfsError) -> vfs_mut_api::Error {
    match error {
        VfsError::NotFound(_) => vfs_mut_api::Error::NotFound,
        VfsError::AlreadyExists(_) => vfs_mut_api::Error::AlreadyExists,
        VfsError::NotEmpty(_) => vfs_mut_api::Error::NotEmpty,
        VfsError::Conflict { .. } => vfs_mut_api::Error::Conflict,
        VfsError::InvalidPath(_) | VfsError::InvalidContentId(_) => vfs_mut_api::Error::Invalid,
        VfsError::ReadOnly | VfsError::RoleMismatch { .. } => vfs_mut_api::Error::Denied,
        VfsError::Leased { .. } => vfs_mut_api::Error::Conflict,
        VfsError::Fenced { .. } | VfsError::SnapshotRefused { .. } => {
            vfs_mut_api::Error::Conflict
        }
        VfsError::NotHydrated(_)
        | VfsError::Transport(_)
        | VfsError::Io(_)
        | VfsError::Sql(_)
        | VfsError::Config(_)
        | VfsError::NoDataDirectory
        | VfsError::CorruptMeta { .. }
        | VfsError::GenerationExhausted => vfs_mut_api::Error::Io,
    }
}

pub fn add_mutation_to_linker<T>(linker: &mut Linker<T>) -> anyhow::Result<()>
where
    T: vfs_mut_api::Host + 'static,
{
    vfs_mut_api::add_to_linker::<T, HasSelf<T>>(linker, |state| state)?;
    Ok(())
}

pub fn add_to_linker<T>(linker: &mut Linker<T>) -> anyhow::Result<()>
where
    T: vfs::Host + index::Host + cid::Host + 'static,
{
    vfs::add_to_linker::<T, HasSelf<T>>(linker, |state| state)?;
    cid::add_to_linker::<T, HasSelf<T>>(linker, |state| state)?;
    index::add_to_linker::<T, HasSelf<T>>(linker, |state| state)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use self::soil::index::index::Host as _;
    use self::soil::vfs::vfs::Host as _;
    use std::path::Path;

    fn seed(root: &Path) {
        let mut vfs = Vfs::open(root).unwrap();
        vfs.put("notes.txt", b"hello greece trip", "text/plain", Expect::Any)
            .unwrap();
        vfs.put("photos/greece-01.jpg", b"jpeg-one", "image/jpeg", Expect::Any)
            .unwrap();
        vfs.put("photos/greece-02.jpg", b"jpeg-two", "image/jpeg", Expect::Any)
            .unwrap();
    }

    #[test]
    fn streams_a_write_into_the_namespace() {
        let dir = tempfile::tempdir().unwrap();
        let host = VfsHost::open(dir.path()).unwrap();
        let mut writer = host
            .begin_writer(
                "captures/test/audio.pcm".into(),
                "application/octet-stream".into(),
                Some(11),
                None,
                None,
            )
            .unwrap();
        assert_eq!(writer.write(b"hello ").unwrap(), 6);
        assert_eq!(writer.write(b"world").unwrap(), 11);
        let result = writer.finish().unwrap();
        assert_eq!(result.path, "captures/test/audio.pcm");
        assert_eq!(result.size, 11);

        let mut host = VfsHost::open(dir.path()).unwrap();
        let chunk = <VfsHost as vfs::Host>::read(
            &mut host,
            "captures/test/audio.pcm".into(),
            0,
            MAX_RANGE,
        )
        .unwrap();
        assert_eq!(chunk.data, b"hello world");
    }

    #[test]
    fn abort_removes_the_staged_write() {
        let dir = tempfile::tempdir().unwrap();
        let host = VfsHost::open(dir.path()).unwrap();
        let mut writer = host
            .begin_writer("aborted.bin".into(), "application/octet-stream".into(), None, None, None)
            .unwrap();
        writer.write(b"never committed").unwrap();
        writer.abort().unwrap();
        let mut host = VfsHost::open(dir.path()).unwrap();
        assert!(matches!(
            <VfsHost as vfs::Host>::stat(&mut host, "aborted.bin".into()),
            Err(vfs::Error::NotFound)
        ));
    }

    #[test]
    fn lists_and_searches_the_derived_index() {
        let dir = tempfile::tempdir().unwrap();
        seed(dir.path());
        let mut host = VfsHost::open(dir.path()).unwrap();

        let root = host.stat(String::new()).unwrap();
        assert_eq!(root.kind, vfs::Kind::Directory);
        assert_eq!(root.child_count, 2);

        let page = host.ls(String::new(), 0, 10).unwrap();
        assert_eq!(page.entries.len(), 2);
        assert_eq!(page.entries[0].path, "notes.txt");
        assert_eq!(page.entries[1].path, "photos");

        let page = host
            .search(index::Query {
                text: "greece".into(),
                mode: None,
                parent: None,
                media_type_prefix: None,
                offset: 0,
                limit: 10,
            })
            .unwrap();
        assert_eq!(page.hits.len(), 3);
        assert!(
            page.hits
                .iter()
                .all(|hit| hit.path.contains("greece") || hit.path == "notes.txt")
        );
    }

    #[test]
    fn explicit_empty_directories_are_browsable() {
        let dir = tempfile::tempdir().unwrap();
        {
            let mut vfs = Vfs::open(dir.path()).unwrap();
            vfs.mkdir("empty").unwrap();
            vfs.mkdir("empty/nested").unwrap();
        }
        let mut host = VfsHost::open(dir.path()).unwrap();
        let page = host.ls("empty".into(), 0, 10).unwrap();
        assert_eq!(page.entries.len(), 1);
        assert_eq!(page.entries[0].path, "empty/nested");
        assert_eq!(page.entries[0].kind, vfs::Kind::Directory);
        let entry = host.stat("empty".into()).unwrap();
        assert_eq!(entry.kind, vfs::Kind::Directory);
        assert_eq!(entry.child_count, 1);
    }

    #[test]
    fn reads_ranges_and_reports_eof() {
        let dir = tempfile::tempdir().unwrap();
        seed(dir.path());
        let mut host = VfsHost::open(dir.path()).unwrap();
        let chunk = host.read("notes.txt".into(), 6, 6).unwrap();
        assert_eq!(chunk.data, b"greece");
        assert!(!chunk.eof);
        let tail = host.read("notes.txt".into(), 12, 64).unwrap();
        assert_eq!(tail.data, b" trip");
        assert!(tail.eof);
        assert_eq!(
            tail.content_id,
            fungos_vfs::ContentId::from_bytes(b"hello greece trip").to_string()
        );
    }

    #[test]
    fn a_listing_sees_a_write_from_another_host() {
        let dir = tempfile::tempdir().unwrap();
        let mut host = VfsHost::open(dir.path()).unwrap();
        {
            // A second host updates the shared index too, which is the case
            // that exposed the stale in-memory cache.
            let other = VfsHost::open(dir.path()).unwrap();
            let mut writer = other
                .begin_writer(
                    "later.txt".into(),
                    "text/plain".into(),
                    None,
                    None,
                    None,
                )
                .unwrap();
            writer.write(b"from ingest").unwrap();
            writer.finish().unwrap();
        }
        let page = host.ls("".into(), 0, 10).unwrap();
        assert!(page.entries.iter().any(|entry| entry.path == "later.txt"));
        let chunk = host.read("later.txt".into(), 0, 64).unwrap();
        assert_eq!(chunk.data, b"from ingest");
    }

    #[test]
    fn browse_orders_by_size_and_paginates() {
        let dir = tempfile::tempdir().unwrap();
        seed(dir.path());
        let mut host = VfsHost::open(dir.path()).unwrap();
        let page = host
            .browse("photos".into(), index::Order::SizeDesc, 0, 1)
            .unwrap();
        assert_eq!(page.hits.len(), 1);
        assert_eq!(page.hits[0].path, "photos/greece-01.jpg");
        assert_eq!(page.next, Some(1));
    }

    #[test]
    fn regex_search_indexes_code_extensions_and_fuzzy_text() {
        let dir = tempfile::tempdir().unwrap();
        let mut vfs = Vfs::open(dir.path()).unwrap();
        vfs.put(
            "src/main.rs",
            b"fn main() {\n    run();\n}\n",
            "application/octet-stream",
            Expect::Any,
        )
        .unwrap();
        let mut host = VfsHost::open(dir.path()).unwrap();
        let page = host
            .search(index::Query {
                text: r"fn\s+main".into(),
                mode: Some(index::Mode::Regex),
                parent: None,
                media_type_prefix: None,
                offset: 0,
                limit: 10,
            })
            .unwrap();
        assert_eq!(page.hits.len(), 1);
        assert_eq!(page.hits[0].path, "src/main.rs");
        assert!(
            page.hits[0]
                .snippet
                .as_deref()
                .is_some_and(|s| s.contains("fn main"))
        );

        let page = host
            .search(index::Query {
                text: "fn main".into(),
                mode: Some(index::Mode::Fuzzy),
                parent: None,
                media_type_prefix: None,
                offset: 0,
                limit: 10,
            })
            .unwrap();
        assert!(page.hits.iter().any(|hit| hit.path == "src/main.rs"));
    }

    #[test]
    fn full_text_search_uses_fts_and_can_filter_by_parent() {
        let dir = tempfile::tempdir().unwrap();
        seed(dir.path());
        let mut host = VfsHost::open(dir.path()).unwrap();
        let page = host
            .search(index::Query {
                text: "trip".into(),
                mode: Some(index::Mode::FullText),
                parent: None,
                media_type_prefix: None,
                offset: 0,
                limit: 10,
            })
            .unwrap();
        assert_eq!(page.hits.len(), 1);
        assert_eq!(page.hits[0].path, "notes.txt");
        assert_eq!(page.hits[0].matched_by, Some(index::MatchSource::Text));

        let page = host
            .search(index::Query {
                text: "trip".into(),
                mode: Some(index::Mode::FullText),
                parent: Some("photos".into()),
                media_type_prefix: None,
                offset: 0,
                limit: 10,
            })
            .unwrap();
        assert!(page.hits.is_empty());
    }

    #[test]
    fn persistent_index_survives_reopen_without_rebuilding() {
        let dir = tempfile::tempdir().unwrap();
        seed(dir.path());
        let index_path = dir.path().join("derived.sqlite3");
        let state = {
            let mut host = VfsHost::open_with_index(dir.path(), &index_path).unwrap();
            host.state().unwrap()
        };
        assert!(index_path.exists());
        let mut host = VfsHost::open_with_index(dir.path(), &index_path).unwrap();
        let reopened = host.state().unwrap();
        assert_eq!(reopened.generation, state.generation);
        assert_eq!(reopened.entries, state.entries);

        let mut vfs = Vfs::open(dir.path()).unwrap();
        vfs.put("notes/new.txt", b"a new semantic note", "text/plain", Expect::Any)
            .unwrap();
        let state = host.refresh_index().unwrap();
        assert_eq!(state.generation, 2);
        let page = host
            .search(index::Query {
                text: "semantic".into(),
                mode: Some(index::Mode::FullText),
                parent: None,
                media_type_prefix: None,
                offset: 0,
                limit: 10,
            })
            .unwrap();
        assert_eq!(page.hits[0].path, "notes/new.txt");
    }

    struct FakeEmbeddings;

    impl EmbeddingProvider for FakeEmbeddings {
        fn model_id(&self) -> &str {
            "fake:test"
        }
        fn dimension(&self) -> usize {
            2
        }
        fn embed(&self, texts: &[String]) -> anyhow::Result<Vec<Vec<f32>>> {
            Ok(texts
                .iter()
                .map(|text| {
                    if text.to_lowercase().contains("trip") {
                        vec![1.0, 0.0]
                    } else {
                        vec![0.0, 1.0]
                    }
                })
                .collect())
        }
    }

    #[test]
    fn semantic_search_uses_the_host_embedding_provider() {
        let dir = tempfile::tempdir().unwrap();
        seed(dir.path());
        let mut host = VfsHost::open(dir.path())
            .unwrap()
            .with_embeddings(Arc::new(FakeEmbeddings))
            .unwrap();
        let page = host
            .search(index::Query {
                text: "trip".into(),
                mode: Some(index::Mode::Semantic),
                parent: None,
                media_type_prefix: None,
                offset: 0,
                limit: 10,
            })
            .unwrap();
        assert_eq!(page.hits.len(), 1);
        assert_eq!(page.hits[0].path, "notes.txt");
        assert_eq!(page.hits[0].matched_by, Some(index::MatchSource::Embedding));
    }

    #[test]
    fn knn_returns_the_same_nearest_chunk_paths() {
        let dir = tempfile::tempdir().unwrap();
        seed(dir.path());
        let mut host = VfsHost::open(dir.path())
            .unwrap()
            .with_embeddings(Arc::new(FakeEmbeddings))
            .unwrap();
        let page = host
            .knn(index::KnnQuery {
                text: "trip".into(),
                parent: None,
                media_type_prefix: None,
                offset: 0,
                limit: 10,
            })
            .unwrap();
        assert_eq!(page.hits.len(), 1);
        assert_eq!(page.hits[0].path, "notes.txt");
        assert_eq!(page.hits[0].matched_by, Some(index::MatchSource::Embedding));
    }

    #[test]
    fn semantic_search_is_an_explicit_unsupported_error() {
        let dir = tempfile::tempdir().unwrap();
        seed(dir.path());
        let mut host = VfsHost::open(dir.path()).unwrap();
        let error = host
            .search(index::Query {
                text: "trip".into(),
                mode: Some(index::Mode::Semantic),
                parent: None,
                media_type_prefix: None,
                offset: 0,
                limit: 10,
            })
            .unwrap_err();
        assert_eq!(error, index::Error::Unsupported);
    }
}
