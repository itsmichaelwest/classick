use crate::atomic_file::AtomicFileWriter;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CacheIndex {
    version: u32,
    sources: BTreeMap<String, Option<String>>,
}

impl Default for CacheIndex {
    fn default() -> Self {
        Self {
            version: 1,
            sources: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ArtworkCache {
    root: PathBuf,
    writer: AtomicFileWriter,
}

impl ArtworkCache {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            writer: AtomicFileWriter::new(),
        }
    }

    pub fn store(&self, source: &Path, raw_art: &[u8]) -> Result<String> {
        let normalized = crate::artwork::normalize(raw_art)?;
        self.store_normalized(source, &normalized)
    }

    pub fn store_normalized(&self, source: &Path, normalized: &[u8]) -> Result<String> {
        let hash = blake3::hash(normalized).to_hex().to_string();
        self.writer
            .write(&self.art_path(&hash), normalized)
            .context("cache normalized artwork")?;
        self.set_source(source, Some(hash.clone()))?;
        Ok(hash)
    }

    pub fn record_no_art(&self, source: &Path) -> Result<()> {
        self.set_source(source, None)
    }

    pub fn load_hash(&self, hash: &str) -> Result<Vec<u8>> {
        let bytes = std::fs::read(self.art_path(hash))
            .with_context(|| format!("read cached artwork {hash}"))?;
        let actual = blake3::hash(&bytes).to_hex().to_string();
        if actual != hash {
            bail!("cached artwork {hash} failed content validation");
        }
        Ok(bytes)
    }

    pub fn load_for_source(&self, source: &Path) -> Result<Option<Vec<u8>>> {
        let index = self.load_index()?;
        let source_key = source_key(source);
        match index.sources.get(&source_key) {
            Some(Some(hash)) => self.load_hash(hash).map(Some),
            Some(None) => Ok(None),
            None => bail!("artwork was not prepared for {}", source.display()),
        }
    }

    fn set_source(&self, source: &Path, value: Option<String>) -> Result<()> {
        let mut index = self.load_index()?;
        index.sources.insert(source_key(source), value);
        let bytes = serde_json::to_vec_pretty(&index).context("encode artwork cache index")?;
        self.writer
            .write(&self.root.join("index.json"), &bytes)
            .context("write artwork cache index")
    }

    fn load_index(&self) -> Result<CacheIndex> {
        let path = self.root.join("index.json");
        if !path.exists() {
            return Ok(CacheIndex::default());
        }
        let bytes = std::fs::read(&path)
            .with_context(|| format!("read artwork cache index {}", path.display()))?;
        let index: CacheIndex = serde_json::from_slice(&bytes)
            .with_context(|| format!("decode artwork cache index {}", path.display()))?;
        if index.version != 1 {
            bail!("unsupported artwork cache version {}", index.version);
        }
        Ok(index)
    }

    fn art_path(&self, hash: &str) -> PathBuf {
        self.root.join("objects").join(format!("{hash}.jpg"))
    }
}

fn source_key(source: &Path) -> String {
    blake3::hash(source.to_string_lossy().as_bytes())
        .to_hex()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    fn tempdir() -> PathBuf {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target/test-tmp")
            .join(format!("artwork-cache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn content_addressed_cache_deduplicates_and_records_no_art() {
        let cache = ArtworkCache::new(tempdir());
        let first = cache
            .store_normalized(Path::new("a.flac"), b"jpeg")
            .unwrap();
        let second = cache
            .store_normalized(Path::new("b.flac"), b"jpeg")
            .unwrap();
        assert_eq!(first, second);
        assert_eq!(
            cache.load_for_source(Path::new("a.flac")).unwrap(),
            Some(b"jpeg".to_vec())
        );

        cache.record_no_art(Path::new("none.flac")).unwrap();
        assert_eq!(cache.load_for_source(Path::new("none.flac")).unwrap(), None);
        assert!(cache.load_for_source(Path::new("unknown.flac")).is_err());
    }
}
