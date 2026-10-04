/// One completable entry: its id, display text, popularity, alternative names and contexts.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct Document {
    /// Stable id. It also breaks ranking ties, lower first. Keyed documents derive it from their key.
    pub id: u64,
    /// Caller's string id, when the document was created with [`Document::keyed`].
    pub key: Option<String>,
    pub text: String,
    /// Popularity, typically in `[0, 1]`; the more popular rank first, and zero ranks last.
    pub popularity: f32,
    pub aliases: Vec<Alias>,
    /// Tags that completion requests can filter on, e.g. a category or a tenant.
    pub contexts: Vec<String>,
    /// Embedding for vector search. Segments keep it only in quantised form, so documents read
    /// back from a segment carry `None`.
    pub vector: Option<Vec<f32>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Alias {
    pub text: String,
    pub kind: AliasKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum AliasKind {
    /// Prefix-searchable through `complete_aliases`.
    Synonym = 0,
    /// Matched only exactly, by `complete`: acronyms, initialisms, short codes.
    Abbreviation = 1,
}

/// The numeric id of a string key: a stable 64-bit hash, so any process derives the same id.
pub fn key_id(key: &str) -> u64 {
    xxhash_rust::xxh3::xxh3_64(key.as_bytes())
}

impl Document {
    pub fn new(id: u64, text: impl Into<String>, popularity: f32) -> Self {
        Self {
            id,
            key: None,
            text: text.into(),
            popularity,
            aliases: Vec::new(),
            contexts: Vec::new(),
            vector: None,
        }
    }

    /// A document identified by a string key; its id is [`key_id`] of the key.
    pub fn keyed(key: impl Into<String>, text: impl Into<String>, popularity: f32) -> Self {
        let key = key.into();
        Self {
            key: Some(key.clone()),
            ..Self::new(key_id(&key), text, popularity)
        }
    }

    pub fn with_vector(mut self, vector: Vec<f32>) -> Self {
        self.vector = Some(vector);
        self
    }

    pub fn with_alias(mut self, text: impl Into<String>, kind: AliasKind) -> Self {
        self.aliases.push(Alias::new(text, kind));
        self
    }

    pub fn with_synonym(self, text: impl Into<String>) -> Self {
        self.with_alias(text, AliasKind::Synonym)
    }

    pub fn with_abbreviation(self, text: impl Into<String>) -> Self {
        self.with_alias(text, AliasKind::Abbreviation)
    }

    pub fn with_context(mut self, context: impl Into<String>) -> Self {
        self.contexts.push(context.into());
        self
    }
}

impl Alias {
    pub fn new(text: impl Into<String>, kind: AliasKind) -> Self {
        Self {
            text: text.into(),
            kind,
        }
    }
}
