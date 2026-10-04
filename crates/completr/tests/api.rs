use std::sync::Arc;

use completr::{
    key_id, AliasKind, Document, Engine, HybridOptions, Index, IndexOptions, MatchKind,
    SearchOptions, Segment,
};

fn catalog() -> Vec<Document> {
    vec![
        Document::keyed("sku-ml", "Machine Learning", 0.9)
            .with_abbreviation("ML")
            .with_synonym("statistical learning")
            .with_context("books")
            .with_vector(v(&[1.0, 0.0, 0.0, 0.0])),
        Document::keyed("sku-mv", "Machine Vision", 0.4)
            .with_context("courses")
            .with_vector(v(&[0.9, 0.1, 0.0, 0.0])),
        Document::keyed("sku-ds", "Data Science", 0.7)
            .with_synonym("data analytics")
            .with_context("books")
            .with_context("courses")
            .with_vector(v(&[0.0, 1.0, 0.0, 0.0])),
        Document::new(42, "Machine Translation", 0.2).with_vector(v(&[0.8, 0.0, 0.2, 0.0])),
    ]
}

fn v(head: &[f32]) -> Vec<f32> {
    let mut v = head.to_vec();
    v.resize(8, 0.0);
    v
}

fn index() -> Index {
    Index::from_documents(catalog()).unwrap()
}

fn ids<T>(hits: &[T], id: impl Fn(&T) -> u64) -> Vec<u64> {
    hits.iter().map(id).collect()
}

#[test]
fn documents_round_trip_with_keys_and_contexts() {
    let index = index();
    let doc = index.document_by_key("sku-ds").unwrap();
    assert_eq!(doc.id, key_id("sku-ds"));
    assert_eq!(doc.text, "Data Science");
    assert_eq!(doc.contexts, ["books", "courses"]);
    assert_eq!(doc.aliases[0].kind, AliasKind::Synonym);
    assert!(index.document_by_key("sku-none").is_none());
    assert_eq!(index.document(42).unwrap().key, None);
}

#[test]
fn suggestions_carry_text_key_and_highlights() {
    let index = index();
    let hits = index.complete("mach", 10);
    assert_eq!(hits[0].text, "Machine Learning");
    assert_eq!(hits[0].key.as_deref(), Some("sku-ml"));
    assert_eq!(hits[0].highlights.len(), 1);
    assert_eq!(&hits[0].text[hits[0].highlights[0].clone()], "Mach");
    let typo = index.complete("machne lerning", 10);
    assert_eq!(typo[0].kind, MatchKind::Fuzzy);
    assert_eq!(&typo[0].text[typo[0].highlights[1].clone()], "Learning");
    let abbreviation = index.complete("ML", 10);
    assert_eq!(abbreviation[0].kind, MatchKind::Abbreviation);
    assert!(abbreviation[0].highlights.is_empty());
    let alias = index.complete_aliases("data ana", 10);
    assert_eq!(alias[0].text, "Data Science");
}

#[test]
fn contexts_filter_every_search() {
    let index = index();
    let books = SearchOptions::new(10).contexts(["books"]);
    let book_ids = ids(&index.complete_with("mach", &books), |s| s.id);
    assert_eq!(book_ids, [key_id("sku-ml")]);
    // Short queries are cached unfiltered; filtered requests must not read or fill that cache.
    let all = ids(&index.complete("ma", 10), |s| s.id);
    assert_eq!(all.len(), 3);
    assert_eq!(
        ids(&index.complete_with("ma", &books), |s| s.id),
        [key_id("sku-ml")]
    );
    assert_eq!(ids(&index.complete("ma", 10), |s| s.id), all);

    let courses = SearchOptions::new(10).contexts(["courses"]);
    assert_eq!(
        ids(&index.complete_aliases_with("data", &courses), |s| s.id),
        [key_id("sku-ds")]
    );
    assert!(index.complete_aliases_with("stat", &courses).is_empty());
    let semantic = index
        .vector_search_with(&v(&[1.0, 0.0, 0.0, 0.0]), &courses)
        .unwrap();
    assert_eq!(
        ids(&semantic, |s| s.id),
        [key_id("sku-mv"), key_id("sku-ds")]
    );
    let hybrid = index
        .hybrid_search(
            "mach",
            &v(&[1.0, 0.0, 0.0, 0.0]),
            10,
            &HybridOptions::default().contexts(["books"]),
        )
        .unwrap();
    assert!(hybrid
        .iter()
        .all(|h| [key_id("sku-ml"), key_id("sku-ds")].contains(&h.id)));
    let none = SearchOptions::new(10).contexts(["music"]);
    assert!(index.complete_with("mach", &none).is_empty());
}

#[test]
fn contexts_filter_layers_and_newer_versions() {
    let base = Arc::new(Segment::build(catalog(), []).unwrap());
    let delta = Arc::new(
        Segment::build(
            [Document::keyed("sku-mv", "Machine Vision", 0.4).with_context("books")],
            [],
        )
        .unwrap(),
    );
    let index = Index::new(vec![base, delta], IndexOptions::default()).unwrap();
    let books = SearchOptions::new(10).contexts(["books"]);
    let mut found = ids(&index.complete_with("mach", &books), |s| s.id);
    found.sort();
    let mut expected = vec![key_id("sku-ml"), key_id("sku-mv")];
    expected.sort();
    assert_eq!(found, expected);

    let engine = Engine::new();
    engine.publish([("shared".to_owned(), Some(Arc::new(index)))]);
    let layered = engine.complete_with(&["shared", "missing"], "mach", &books);
    assert_eq!(layered.len(), 2);
    assert!(layered.iter().all(|l| l.layer == 0));
}

#[test]
fn key_collisions_are_rejected() {
    let a = Document::keyed("a", "first", 0.1);
    let mut b = Document::keyed("b", "second", 0.1);
    b.id = a.id;
    assert!(Segment::build([a.clone(), b.clone()], []).is_err());
    let older = Arc::new(Segment::build([a], []).unwrap());
    let newer = Arc::new(Segment::build([b], []).unwrap());
    assert!(Index::new(vec![older.clone(), newer], IndexOptions::default()).is_err());
    let same = Arc::new(Segment::build([Document::keyed("a", "first, updated", 0.2)], []).unwrap());
    let updated = Index::new(vec![older, same], IndexOptions::default()).unwrap();
    assert_eq!(updated.document_by_key("a").unwrap().text, "first, updated");
    assert!(Segment::build([Document::keyed("", "empty", 0.1)], []).is_err());
}

#[test]
fn corrections_keep_short_words_and_the_word_being_typed() {
    let index = Index::from_documents([
        Document::new(1, "Welcome to the Jungle", 0.5),
        Document::new(2, "Welcome Home", 0.9),
        Document::new(3, "The secret world of bees", 0.5),
        Document::new(4, "Topology for beginners", 0.9),
    ])
    .unwrap();
    let ids = |q: &str| {
        index
            .complete(q, 10)
            .iter()
            .map(|s| s.id)
            .collect::<Vec<_>>()
    };
    assert_eq!(ids("welcoem to"), [1]);
    assert_eq!(ids("the sercet wor"), [3]);
    let direct = index.complete("welcome to", 10);
    assert_eq!((direct[0].id, direct[0].kind), (1, MatchKind::Prefix));
}

#[test]
fn corrections_survive_a_dropped_letter_and_unindexed_short_words() {
    let index = Index::from_documents([
        Document::new(1, "Machine learning", 0.5),
        Document::new(2, "Kerning", 0.9),
        Document::new(3, "Eigeninitiative", 0.5),
        Document::new(4, "Red teaming", 0.5),
    ])
    .unwrap();
    let top = |q: &str| index.complete(q, 10).first().map(|s| s.id);
    assert_eq!(top("machine lerning"), Some(1));
    assert_eq!(top("eigeniniative"), Some(3));
    assert_eq!(top("AI red teaming"), Some(4));
}

#[test]
fn a_small_layer_scores_on_the_scale_of_the_first() {
    let mut songs = vec![
        Document::keyed("dancing", "Dancing Queen – ABBA", 0.85),
        Document::keyed("ownown", "Dancing On My Own – Robyn", 0.5),
    ];
    songs.extend(
        (0..2_000u64).map(|i| Document::new(i, format!("Song {i}"), (i % 90) as f32 / 100.0)),
    );
    let engine = Engine::new();
    let same = Index::from_documents([Document::keyed("ownown", "Dancing On My Own – Robyn", 0.5)])
        .unwrap();
    engine.publish([
        (
            "catalog".to_owned(),
            Some(Arc::new(Index::from_documents(songs).unwrap())),
        ),
        ("same".to_owned(), Some(Arc::new(same))),
    ]);
    let alone = engine.complete(&["catalog"], "danc", 10);
    let layered = engine.complete(&["catalog", "same"], "danc", 10);
    let scores = |hits: &[completr::LayeredSuggestion<completr::Suggestion>]| {
        hits.iter()
            .map(|h| (h.suggestion.id, (h.suggestion.score * 1e6).round()))
            .collect::<Vec<_>>()
    };
    assert_eq!(scores(&alone), scores(&layered));
    assert_eq!(layered[1].layer, 1);
}

#[test]
fn an_exact_title_comes_first_among_many_longer_ones() {
    let index = Index::from_documents(
        (0..20_000u64).map(|i| Document::new(i, format!("product {i}"), 0.5)),
    )
    .unwrap();
    let hits = index.complete("product 12", 5);
    assert_eq!((hits[0].id, hits[0].kind), (12, MatchKind::Exact));
}

#[test]
fn any_popularity_ranks_above_none() {
    let index = Index::from_documents([
        Document::new(1, "Dubrovnik – A", 0.0),
        Document::new(2, "Dubrovnik – B", 0.01),
    ])
    .unwrap();
    let ids: Vec<u64> = index
        .complete("dubrovnik", 10)
        .iter()
        .map(|s| s.id)
        .collect();
    assert_eq!(ids, [2, 1]);
}

#[test]
fn later_layers_hide_renamed_and_deleted_documents() {
    let shared = Index::from_documents([
        Document::keyed("mv", "Machine Vision", 0.9),
        Document::keyed("ml", "Machine Learning", 0.5),
        Document::keyed("mt", "Machine Translation", 0.4),
    ])
    .unwrap();
    let tenant = Index::new(
        vec![Arc::new(
            Segment::build(
                [Document::keyed("mv", "Computer Vision", 0.9)],
                [key_id("mt")],
            )
            .unwrap(),
        )],
        IndexOptions::default(),
    )
    .unwrap();
    let engine = Engine::new();
    engine.publish([
        ("shared".to_owned(), Some(Arc::new(shared))),
        ("acme".to_owned(), Some(Arc::new(tenant))),
    ]);
    let found: Vec<_> = engine
        .complete(&["shared", "acme"], "machine", 10)
        .into_iter()
        .map(|l| l.suggestion.key.unwrap())
        .collect();
    assert_eq!(found, ["ml"]);
    let vision = engine.complete(&["shared", "acme"], "vision", 10);
    assert_eq!(
        (vision[0].suggestion.text.as_str(), vision[0].layer),
        ("Computer Vision", 1)
    );
}
