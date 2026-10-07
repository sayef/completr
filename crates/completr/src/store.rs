//! Segment and object storage behind a URL: a local path, `file://`, `memory://`, `s3://`, `gs://` or `az://`.

use std::path::{Path as FsPath, PathBuf};
use std::sync::{Arc, OnceLock};

use futures::{StreamExt, TryStreamExt};
use object_store::local::LocalFileSystem;
use object_store::memory::InMemory;
use object_store::path::Path;
use object_store::{ObjectStore, ObjectStoreExt, PutMode, PutOptions, PutPayload, TagSet};
use url::Url;

use crate::{Error, Segment};

#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct ObjectInfo {
    pub key: String,
    pub size: u64,
    pub modified_ms: i64,
}

/// Async access to one prefix of an object store. Keys are `/`-separated and relative to it.
#[derive(Clone, Debug)]
pub struct Store {
    inner: Arc<dyn ObjectStore>,
    prefix: Path,
    local_root: Option<PathBuf>,
    /// Where remote segments are downloaded to and memory-mapped from, per store URL.
    cache: Option<PathBuf>,
    namespace: String,
    /// Tags on every object written, e.g. to select bucket lifecycle rules.
    tags: TagSet,
}

impl Store {
    pub async fn open(url: &str) -> Result<Self, Error> {
        Self::open_with(url, Vec::<(String, String)>::new()).await
    }

    /// `options` are `object_store` config keys (e.g. `aws_access_key_id`, `aws_region`,
    /// `aws_endpoint`) and override anything found in the environment or AWS config files.
    pub async fn open_with<K: Into<String>, V: Into<String>>(
        url: &str,
        options: impl IntoIterator<Item = (K, V)>,
    ) -> Result<Self, Error> {
        let options: Vec<(String, String)> = options
            .into_iter()
            .map(|(k, v)| (k.into(), v.into()))
            .collect();
        if !url.contains("://") {
            return Self::local(FsPath::new(url));
        }
        let parsed =
            Url::parse(url).map_err(|e| Error::input(format!("invalid store url {url:?}: {e}")))?;
        let prefix = Path::from_url_path(parsed.path()).map_err(object_store::Error::from)?;
        let inner: Arc<dyn ObjectStore> = match parsed.scheme() {
            "file" => {
                let path = parsed
                    .to_file_path()
                    .map_err(|_| Error::input(format!("invalid file url {url:?}")))?;
                return Self::local(&path);
            }
            "memory" => Arc::new(InMemory::new()),
            #[cfg(feature = "aws")]
            "s3" | "s3a" => s3::build(&parsed, options).await?,
            _ => object_store::parse_url_opts(&parsed, options)?.0.into(),
        };
        let namespace = url
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || "._-".contains(c) {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        Ok(Self {
            inner,
            prefix,
            local_root: None,
            cache: None,
            namespace,
            tags: TagSet::default(),
        })
    }

    fn local(path: &FsPath) -> Result<Self, Error> {
        std::fs::create_dir_all(path)?;
        let root = path.canonicalize()?;
        let inner = Arc::new(LocalFileSystem::new_with_prefix(&root)?);
        Ok(Self {
            inner,
            prefix: Path::default(),
            local_root: Some(root),
            cache: None,
            namespace: String::new(),
            tags: TagSet::default(),
        })
    }

    fn path(&self, key: &str) -> Path {
        let key = key.trim_matches('/');
        match (self.prefix.as_ref().is_empty(), key.is_empty()) {
            (true, _) => Path::from(key),
            (false, true) => self.prefix.clone(),
            (false, false) => Path::from(format!("{}/{key}", self.prefix)),
        }
    }

    pub async fn get(&self, key: &str) -> Result<Vec<u8>, Error> {
        Ok(self
            .inner
            .get(&self.path(key))
            .await?
            .bytes()
            .await?
            .to_vec())
    }

    /// Tags every object this store writes from now on; S3 and Azure keep them, other stores ignore them.
    pub fn with_tags<K: AsRef<str>, V: AsRef<str>>(
        mut self,
        tags: impl IntoIterator<Item = (K, V)>,
    ) -> Self {
        let mut set = TagSet::default();
        for (key, value) in tags {
            set.push(key.as_ref(), value.as_ref());
        }
        self.tags = set;
        self
    }

    /// The tags written with every object, URL-encoded as S3 takes them.
    pub fn tags(&self) -> &str {
        self.tags.encoded()
    }

    pub async fn put(&self, key: &str, data: Vec<u8>) -> Result<(), Error> {
        let options = PutOptions {
            tags: self.tags.clone(),
            ..PutOptions::default()
        };
        self.inner
            .put_opts(&self.path(key), PutPayload::from(data), options)
            .await?;
        Ok(())
    }

    /// Writes only if `key` does not exist yet, atomically (S3 `If-None-Match: *`). Returns
    /// whether it wrote.
    pub async fn put_if_absent(&self, key: &str, data: Vec<u8>) -> Result<bool, Error> {
        let options = PutOptions {
            mode: PutMode::Create,
            tags: self.tags.clone(),
            ..PutOptions::default()
        };
        match self
            .inner
            .put_opts(&self.path(key), PutPayload::from(data), options)
            .await
        {
            Ok(_) => Ok(true),
            Err(object_store::Error::AlreadyExists { .. }) => Ok(false),
            Err(e) => Err(e.into()),
        }
    }

    pub async fn delete(&self, key: &str) -> Result<(), Error> {
        self.inner.delete(&self.path(key)).await?;
        Ok(())
    }

    /// Deletes many keys, in bulk requests where the store supports them (S3: 1,000 per call).
    /// Missing keys are not an error.
    pub async fn delete_many(&self, keys: impl IntoIterator<Item = &str>) -> Result<(), Error> {
        let paths: Vec<object_store::Result<Path>> =
            keys.into_iter().map(|k| Ok(self.path(k))).collect();
        let results = self
            .inner
            .delete_stream(futures::stream::iter(paths).boxed());
        let outcomes: Vec<object_store::Result<Path>> = results.collect().await;
        for outcome in outcomes {
            match outcome {
                Ok(_) | Err(object_store::Error::NotFound { .. }) => {}
                Err(e) => return Err(e.into()),
            }
        }
        Ok(())
    }

    /// Keys under `prefix`, sorted.
    pub async fn list(&self, prefix: &str) -> Result<Vec<String>, Error> {
        Ok(self
            .list_objects(prefix)
            .await?
            .into_iter()
            .map(|o| o.key)
            .collect())
    }

    /// Keys under `prefix` with their size and modification time, sorted by key.
    pub async fn list_objects(&self, prefix: &str) -> Result<Vec<ObjectInfo>, Error> {
        self.collect(self.inner.list(Some(&self.path(prefix))))
            .await
    }

    /// Like [`Store::list_objects`], but only keys sorting after `after`.
    pub async fn list_objects_after(
        &self,
        prefix: &str,
        after: &str,
    ) -> Result<Vec<ObjectInfo>, Error> {
        self.collect(
            self.inner
                .list_with_offset(Some(&self.path(prefix)), &self.path(after)),
        )
        .await
    }

    async fn collect(
        &self,
        listing: futures::stream::BoxStream<
            'static,
            object_store::Result<object_store::ObjectMeta>,
        >,
    ) -> Result<Vec<ObjectInfo>, Error> {
        let metas: Vec<_> = listing.try_collect().await?;
        let prefix = self.prefix.as_ref();
        // Keys not strictly under the prefix, such as a console-created folder marker, are skipped.
        let relative = |location: &str| match prefix {
            "" => Some(location.to_owned()),
            _ => location
                .strip_prefix(prefix)?
                .strip_prefix('/')
                .map(str::to_owned),
        };
        let mut objects: Vec<ObjectInfo> = metas
            .into_iter()
            .filter_map(|m| {
                Some(ObjectInfo {
                    key: relative(m.location.as_ref())?,
                    size: m.size,
                    modified_ms: m.last_modified.timestamp_millis(),
                })
            })
            .collect();
        objects.sort_by(|a, b| a.key.cmp(&b.key));
        Ok(objects)
    }

    /// The store's own clock, read from the timestamp of a probe object, so age checks do not
    /// depend on this machine's clock.
    pub async fn now_ms(&self) -> Result<i64, Error> {
        let key = format!("_clock/{}", uuid::Uuid::new_v4().simple());
        self.put(&key, Vec::new()).await?;
        let modified = self
            .inner
            .head(&self.path(&key))
            .await?
            .last_modified
            .timestamp_millis();
        self.delete(&key).await?;
        Ok(modified)
    }

    pub async fn put_segment(&self, key: &str, segment: &Segment) -> Result<(), Error> {
        self.put(key, segment.to_bytes()).await
    }

    /// Loads a segment; local files are memory-mapped rather than read.
    /// Caches remote segments in `dir`, so they are memory-mapped rather than held in memory.
    /// Has no effect on local stores.
    pub fn with_cache_dir(mut self, dir: impl AsRef<FsPath>) -> Result<Self, Error> {
        if self.local_root.is_none() {
            let dir = dir.as_ref().join(&self.namespace);
            std::fs::create_dir_all(&dir)?;
            self.cache = Some(dir.canonicalize()?);
        }
        Ok(self)
    }

    /// Loads a segment. Local and cached files are memory-mapped rather than read.
    pub async fn get_segment(&self, key: &str) -> Result<Segment, Error> {
        if let Some(root) = &self.local_root {
            let path = root.join(self.path(key).as_ref());
            if !path.is_file() {
                return Err(Error::NotFound(key.to_owned()));
            }
            return Segment::open(path);
        }
        let Some(cache) = &self.cache else {
            return Segment::from_bytes(self.get(key).await?);
        };
        let meta = self.inner.head(&self.path(key)).await?;
        let tag: String = meta
            .e_tag
            .as_deref()
            .unwrap_or("none")
            .chars()
            .filter(char::is_ascii_alphanumeric)
            .collect();
        self.cached_segment(cache, key, &tag, None).await
    }

    /// Like [`Store::get_segment`] for a key that is never overwritten, so a cached copy is used
    /// without asking the store. `size`, if known, must match the cached file.
    pub async fn get_immutable_segment(
        &self,
        key: &str,
        size: Option<u64>,
    ) -> Result<Segment, Error> {
        match &self.cache {
            Some(cache) if self.local_root.is_none() => {
                self.cached_segment(cache, key, "immutable", size).await
            }
            _ => self.get_segment(key).await,
        }
    }

    async fn cached_segment(
        &self,
        cache: &FsPath,
        key: &str,
        tag: &str,
        size: Option<u64>,
    ) -> Result<Segment, Error> {
        let location = self.path(key);
        let path = cache.join(format!("{}.{tag}", key.trim_matches('/')));
        if path.is_file() {
            let size_ok = size.is_none_or(|s| std::fs::metadata(&path).is_ok_and(|m| m.len() == s));
            match Segment::open(&path).and_then(|s| s.verify().map(|()| s)) {
                Ok(segment) if size_ok => return Ok(segment),
                _ => std::fs::remove_file(&path)?,
            }
        }
        std::fs::create_dir_all(path.parent().unwrap_or(cache))?;
        let partial = path.with_extension(format!("{}.partial", uuid::Uuid::new_v4().simple()));
        let download = async {
            let mut file = std::fs::File::create(&partial)?;
            let mut stream = self.inner.get(&location).await?.into_stream();
            while let Some(chunk) = stream.try_next().await? {
                std::io::Write::write_all(&mut file, &chunk)?;
            }
            file.sync_all()?;
            std::fs::rename(&partial, &path)?;
            Ok::<_, Error>(())
        };
        if let Err(e) = download.await {
            let _ = std::fs::remove_file(&partial);
            return Err(e);
        }
        let segment = Segment::open(&path)?;
        segment.verify()?;
        Ok(segment)
    }

    /// Deletes cached files except those of `keep`; returns how many it deleted. Mapped files
    /// stay readable after deletion on Unix.
    pub fn prune_cache<'a>(&self, keep: impl IntoIterator<Item = &'a str>) -> Result<usize, Error> {
        let Some(cache) = &self.cache else {
            return Ok(0);
        };
        let keep: std::collections::HashSet<String> = keep
            .into_iter()
            .map(|k| k.trim_matches('/').to_owned())
            .collect();
        let mut removed = 0;
        let mut dirs = vec![cache.clone()];
        while let Some(dir) = dirs.pop() {
            for entry in std::fs::read_dir(&dir)? {
                let path = entry?.path();
                if path.is_dir() {
                    dirs.push(path);
                    continue;
                }
                let relative = path
                    .strip_prefix(cache)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .replace('\\', "/");
                let key = relative
                    .rsplit_once('.')
                    .map_or(relative.as_str(), |(k, _)| k);
                if relative.ends_with(".partial") || !keep.contains(key) {
                    std::fs::remove_file(&path)?;
                    removed += 1;
                }
            }
        }
        Ok(removed)
    }
}

/// Runs a future to completion on completr's internal runtime. Do not call from async code.
pub fn block_on<F: std::future::Future>(future: F) -> F::Output {
    runtime().block_on(future)
}

fn runtime() -> &'static tokio::runtime::Runtime {
    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("completr-store")
            .enable_all()
            .build()
            .expect("failed to start the storage runtime")
    })
}

/// [`Store`] for synchronous callers, run on an internal runtime. Do not call from async code.
#[derive(Clone, Debug)]
pub struct BlockingStore(Store);

impl BlockingStore {
    pub fn open(url: &str) -> Result<Self, Error> {
        runtime().block_on(Store::open(url)).map(Self)
    }

    pub fn open_with<K: Into<String>, V: Into<String>>(
        url: &str,
        options: impl IntoIterator<Item = (K, V)>,
    ) -> Result<Self, Error> {
        runtime().block_on(Store::open_with(url, options)).map(Self)
    }

    pub fn get(&self, key: &str) -> Result<Vec<u8>, Error> {
        runtime().block_on(self.0.get(key))
    }

    pub fn put(&self, key: &str, data: Vec<u8>) -> Result<(), Error> {
        runtime().block_on(self.0.put(key, data))
    }

    pub fn put_if_absent(&self, key: &str, data: Vec<u8>) -> Result<bool, Error> {
        runtime().block_on(self.0.put_if_absent(key, data))
    }

    pub fn delete(&self, key: &str) -> Result<(), Error> {
        runtime().block_on(self.0.delete(key))
    }

    pub fn list(&self, prefix: &str) -> Result<Vec<String>, Error> {
        runtime().block_on(self.0.list(prefix))
    }

    pub fn put_segment(&self, key: &str, segment: &Segment) -> Result<(), Error> {
        runtime().block_on(self.0.put_segment(key, segment))
    }

    pub fn get_segment(&self, key: &str) -> Result<Segment, Error> {
        runtime().block_on(self.0.get_segment(key))
    }

    pub fn with_cache_dir(self, dir: impl AsRef<FsPath>) -> Result<Self, Error> {
        self.0.with_cache_dir(dir).map(Self)
    }

    pub fn with_tags<K: AsRef<str>, V: AsRef<str>>(
        self,
        tags: impl IntoIterator<Item = (K, V)>,
    ) -> Self {
        Self(self.0.with_tags(tags))
    }

    pub fn prune_cache<'a>(&self, keep: impl IntoIterator<Item = &'a str>) -> Result<usize, Error> {
        self.0.prune_cache(keep)
    }

    pub fn get_immutable_segment(&self, key: &str, size: Option<u64>) -> Result<Segment, Error> {
        runtime().block_on(self.0.get_immutable_segment(key, size))
    }

    pub fn inner(&self) -> &Store {
        &self.0
    }
}

#[cfg(feature = "aws")]
mod s3 {
    use std::sync::Arc;

    use object_store::aws::{AmazonS3Builder, AmazonS3ConfigKey};
    use object_store::ObjectStore;
    use url::Url;

    use crate::Error;

    pub(super) async fn build(
        url: &Url,
        options: Vec<(String, String)>,
    ) -> Result<Arc<dyn ObjectStore>, Error> {
        let mut builder = AmazonS3Builder::from_env().with_url(url.as_str());
        for (key, value) in options {
            builder = builder.with_config(key.parse()?, value);
        }
        // Explicit or environment keys win; otherwise the AWS SDK chain resolves them like boto3.
        #[cfg(feature = "aws-credentials")]
        if builder
            .get_config_value(&AmazonS3ConfigKey::AccessKeyId)
            .is_none()
        {
            builder = sdk::attach(builder).await?;
        }
        if builder
            .get_config_value(&AmazonS3ConfigKey::Region)
            .is_none()
        {
            let bucket = url.host_str().unwrap_or_default();
            let region =
                object_store::aws::resolve_bucket_region(bucket, &Default::default()).await?;
            builder = builder.with_region(region);
        }
        Ok(Arc::new(builder.build()?))
    }

    #[cfg(feature = "aws-credentials")]
    mod sdk {
        use std::sync::{Arc, Mutex};
        use std::time::{Duration, SystemTime};

        use aws_config::BehaviorVersion;
        use aws_credential_types::provider::{ProvideCredentials, SharedCredentialsProvider};
        use object_store::aws::{AmazonS3Builder, AmazonS3ConfigKey, AwsCredential};
        use object_store::CredentialProvider;

        use crate::Error;

        /// Adds the AWS SDK default credential chain, and its region if none is configured.
        pub(super) async fn attach(mut builder: AmazonS3Builder) -> Result<AmazonS3Builder, Error> {
            let config = aws_config::defaults(BehaviorVersion::latest()).load().await;
            if let Some(provider) = config.credentials_provider() {
                builder = builder.with_credentials(Arc::new(SdkCredentials {
                    provider,
                    cached: Mutex::default(),
                }));
            }
            if let (None, Some(region)) = (
                builder.get_config_value(&AmazonS3ConfigKey::Region),
                config.region(),
            ) {
                builder = builder.with_region(region.to_string());
            }
            Ok(builder)
        }

        type Cached = Option<(Arc<AwsCredential>, Option<SystemTime>)>;

        #[derive(Debug)]
        struct SdkCredentials {
            provider: SharedCredentialsProvider,
            cached: Mutex<Cached>,
        }

        #[async_trait::async_trait]
        impl CredentialProvider for SdkCredentials {
            type Credential = AwsCredential;

            async fn get_credential(&self) -> object_store::Result<Arc<AwsCredential>> {
                let fresh_until = SystemTime::now() + Duration::from_secs(300);
                if let Some((credential, expiry)) =
                    &*self.cached.lock().unwrap_or_else(|e| e.into_inner())
                {
                    if expiry.is_none_or(|e| e > fresh_until) {
                        return Ok(credential.clone());
                    }
                }
                let resolved = self.provider.provide_credentials().await.map_err(|e| {
                    object_store::Error::Generic {
                        store: "S3",
                        source: Box::new(e),
                    }
                })?;
                let credential = Arc::new(AwsCredential {
                    key_id: resolved.access_key_id().to_owned(),
                    secret_key: resolved.secret_access_key().to_owned(),
                    token: resolved.session_token().map(str::to_owned),
                });
                *self.cached.lock().unwrap_or_else(|e| e.into_inner()) =
                    Some((credential.clone(), resolved.expiry()));
                Ok(credential)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Document;

    /// An in-memory store that records the tags each write carries.
    #[derive(Debug, Default)]
    struct Recording {
        inner: InMemory,
        tags: std::sync::Mutex<Vec<String>>,
    }

    impl std::fmt::Display for Recording {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "Recording")
        }
    }

    #[async_trait::async_trait]
    impl ObjectStore for Recording {
        async fn put_opts(
            &self,
            location: &Path,
            payload: PutPayload,
            opts: PutOptions,
        ) -> object_store::Result<object_store::PutResult> {
            self.tags
                .lock()
                .unwrap()
                .push(opts.tags.encoded().to_owned());
            self.inner.put_opts(location, payload, opts).await
        }

        async fn put_multipart_opts(
            &self,
            location: &Path,
            opts: object_store::PutMultipartOptions,
        ) -> object_store::Result<Box<dyn object_store::MultipartUpload>> {
            self.inner.put_multipart_opts(location, opts).await
        }

        async fn get_opts(
            &self,
            location: &Path,
            options: object_store::GetOptions,
        ) -> object_store::Result<object_store::GetResult> {
            self.inner.get_opts(location, options).await
        }

        fn delete_stream(
            &self,
            locations: futures::stream::BoxStream<'static, object_store::Result<Path>>,
        ) -> futures::stream::BoxStream<'static, object_store::Result<Path>> {
            self.inner.delete_stream(locations)
        }

        fn list(
            &self,
            prefix: Option<&Path>,
        ) -> futures::stream::BoxStream<'static, object_store::Result<object_store::ObjectMeta>>
        {
            self.inner.list(prefix)
        }

        async fn list_with_delimiter(
            &self,
            prefix: Option<&Path>,
        ) -> object_store::Result<object_store::ListResult> {
            self.inner.list_with_delimiter(prefix).await
        }

        async fn copy_opts(
            &self,
            from: &Path,
            to: &Path,
            options: object_store::CopyOptions,
        ) -> object_store::Result<()> {
            self.inner.copy_opts(from, to, options).await
        }
    }

    #[tokio::test]
    async fn every_write_carries_the_tags() {
        let recording = Arc::new(Recording::default());
        let store = Store {
            inner: recording.clone(),
            prefix: Path::default(),
            local_root: None,
            cache: None,
            namespace: "recording".into(),
            tags: TagSet::default(),
        }
        .with_tags([
            ("LifecycleRule", "KeepForever"),
            ("team", "search & ranking"),
        ]);
        assert_eq!(
            store.tags(),
            "LifecycleRule=KeepForever&team=search+%26+ranking"
        );

        let database = crate::Database::new(store);
        let mut txn = database.begin().await.unwrap();
        txn.append_documents("songs", [Document::new(1, "Rust", 0.5)], [])
            .unwrap();
        txn.commit().await.unwrap();
        let mut changes = crate::ChangeSet::new();
        changes.upsert("songs", [Document::new(2, "Go", 0.5)]);
        database.submit(changes).await.unwrap();
        database
            .acquire_lease("ingestor", "me", std::time::Duration::from_secs(60))
            .await
            .unwrap();

        let written = recording.tags.lock().unwrap().clone();
        // A segment, a version, a change set and a lease.
        assert!(written.len() >= 4, "{written:?}");
        assert!(written
            .iter()
            .all(|t| t == "LifecycleRule=KeepForever&team=search+%26+ranking"));
    }

    #[test]
    fn local_and_memory_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let local = dir.path().join("index").to_string_lossy().into_owned();
        let file_url = format!("file://{}", dir.path().join("other").display());
        for url in [local.as_str(), file_url.as_str(), "memory:///base/prefix"] {
            let store = BlockingStore::open(url).unwrap();
            let segment = Segment::build([Document::new(1, "Rust", 0.5)], [7]).unwrap();
            store.put_segment("segments/a.seg", &segment).unwrap();
            let loaded = store.get_segment("segments/a.seg").unwrap();
            assert_eq!(
                loaded.documents().collect::<Vec<_>>(),
                segment.documents().collect::<Vec<_>>()
            );
            assert!(store
                .put_if_absent("_versions/1.json", b"{}".to_vec())
                .unwrap());
            assert!(!store
                .put_if_absent("_versions/1.json", b"{}".to_vec())
                .unwrap());
            assert_eq!(
                store.list("").unwrap(),
                ["_versions/1.json", "segments/a.seg"]
            );
            assert_eq!(store.list("segments").unwrap(), ["segments/a.seg"]);
            store.delete("segments/a.seg").unwrap();
            assert!(matches!(
                store.get_segment("segments/a.seg"),
                Err(Error::NotFound(_))
            ));
        }
    }

    #[test]
    fn caches_remote_segments_by_etag() {
        let dir = tempfile::tempdir().unwrap();
        let store = BlockingStore::open("memory:///bucket/prefix")
            .unwrap()
            .with_cache_dir(dir.path())
            .unwrap();
        let v1 = Segment::build([Document::new(1, "Rust", 0.5)], []).unwrap();
        let v2 = Segment::build([Document::new(2, "Python", 0.5)], []).unwrap();
        store.put_segment("segments/a.seg", &v1).unwrap();
        assert_eq!(
            store
                .get_segment("segments/a.seg")
                .unwrap()
                .ids()
                .collect::<Vec<_>>(),
            [1]
        );
        let cached = || walk(dir.path());
        assert_eq!(cached().len(), 1);
        assert_eq!(
            store
                .get_segment("segments/a.seg")
                .unwrap()
                .ids()
                .collect::<Vec<_>>(),
            [1]
        );
        store.put_segment("segments/a.seg", &v2).unwrap();
        assert_eq!(
            store
                .get_segment("segments/a.seg")
                .unwrap()
                .ids()
                .collect::<Vec<_>>(),
            [2]
        );
        assert_eq!(cached().len(), 2);
        store.put_segment("segments/b.seg", &v1).unwrap();
        store.get_segment("segments/b.seg").unwrap();
        assert_eq!(store.prune_cache(["segments/b.seg"]).unwrap(), 2);
        assert_eq!(cached().len(), 1);

        // Immutable keys skip the etag check, and a cached copy of the wrong size is replaced.
        store.put_segment("segments/c.seg", &v1).unwrap();
        assert_eq!(
            store
                .get_immutable_segment("segments/c.seg", None)
                .unwrap()
                .ids()
                .collect::<Vec<_>>(),
            [1]
        );
        store.put_segment("segments/c.seg", &v2).unwrap();
        assert_eq!(
            store
                .get_immutable_segment("segments/c.seg", None)
                .unwrap()
                .ids()
                .collect::<Vec<_>>(),
            [1]
        );
        let size = Some(v2.size_bytes() as u64 + 1);
        store.put_segment("segments/c.seg", &v2).unwrap();
        assert!(store.get_immutable_segment("segments/c.seg", size).is_ok());
    }

    fn walk(dir: &FsPath) -> Vec<PathBuf> {
        let mut out = Vec::new();
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                out.extend(walk(&path))
            } else {
                out.push(path)
            }
        }
        out
    }
}
