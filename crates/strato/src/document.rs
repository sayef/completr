/// One searchable entry: its caller-assigned id, display text, weight and alternative names.
#[derive(Clone, Debug, PartialEq)]
pub struct Document {
    /// Stable id chosen by the caller. It also breaks ranking ties, lower first.
    pub id: u64,
    pub text: String,
    /// Popularity or prominence, typically in `[0, 1]`. Zero means unknown.
    pub weight: f32,
    pub aliases: Vec<Alias>,
    /// Embedding for `vector_search`. Segments keep it only in quantised form, so documents read
    /// back from a segment carry `None`.
    pub vector: Option<Vec<f32>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Alias {
    pub text: String,
    pub kind: AliasKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AliasKind {
    /// Prefix-searchable through `complete_aliases`.
    Synonym = 0,
    /// Matched only exactly, by `complete`: acronyms, initialisms, short codes.
    Abbreviation = 1,
}

impl Document {
    pub fn new(id: u64, text: impl Into<String>, weight: f32) -> Self {
        Self {
            id,
            text: text.into(),
            weight,
            aliases: Vec::new(),
            vector: None,
        }
    }

    pub fn with_vector(mut self, vector: Vec<f32>) -> Self {
        self.vector = Some(vector);
        self
    }

    pub fn with_alias(mut self, text: impl Into<String>, kind: AliasKind) -> Self {
        self.aliases.push(Alias {
            text: text.into(),
            kind,
        });
        self
    }
}
