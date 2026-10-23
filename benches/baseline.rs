//! Wall-clock baselines for a cold parse and for `Document::apply`.
//!
//! These are measurements, not a latency gate. Unit tests own the
//! correctness checks and this file does not repeat them:
//! `baseline_template_edits_are_not_cold` (none of these edits falls back to
//! a cold parse), `body_edit_reuses_only_the_other_outline` (untouched
//! outlines share green nodes) and
//! `mismatched_underscore_ends_do_not_scale_with_opener_count` (the
//! mismatched `#+end_` case stays linear).

use std::fmt::Write;
use std::hint::black_box;
use std::ops::Range;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use orgcst::{EditSet, TextRange, TextSize, parse};

const SIZES: [usize; 4] = [32, 256, 1024, 4096];

const TODO_LINE: &str = "#+TODO: TODO | DONE\n";
const NEXT_LINE: &str = "#+TODO: TODO NEXT | DONE\n";
const BODY: &str = "body with *bold* and [[https://example.com][link]].\n";
const BODY_WORD: &str = "with";
const BODY_REPLACEMENT: &str = "plus";

struct Generated {
    source: String,
    /// `BODY_WORD` in the middle heading's body.
    body_word: Range<usize>,
}

fn generated(count: usize, keyword_line: &str, middle_keyword: &str) -> Generated {
    let mut source = String::new();
    source.push_str(keyword_line);
    let word_at = BODY
        .find(BODY_WORD)
        .expect("the body template contains the replaced word");
    let middle = count / 2;
    let mut body_word = None;
    for index in 0..count {
        let keyword = if index == middle {
            middle_keyword
        } else {
            "TODO"
        };
        writeln!(source, "* {keyword} [#A] Item {index} :tag:")
            .expect("writing into a String cannot fail");
        if index == middle {
            let start = source.len() + word_at;
            body_word = Some(start..start + BODY_WORD.len());
        }
        source.push_str(BODY);
    }
    let body_word = body_word.expect("the sizes each emit a middle heading");
    assert_eq!(
        &source[body_word.start..body_word.end],
        BODY_WORD,
        "the recorded range is the body word"
    );
    Generated { source, body_word }
}

fn replace(source: &str, range: Range<usize>, replacement: &str) -> EditSet {
    let start = TextSize::try_from(range.start).expect("start fits TextSize");
    let end = TextSize::try_from(range.end).expect("end fits TextSize");
    EditSet::single(source, TextRange::new(start, end), replacement)
        .expect("the range is inside the generated source")
}

fn cold(c: &mut Criterion) {
    let mut group = c.benchmark_group("cold");
    for n in SIZES {
        let src = generated(n, TODO_LINE, "TODO").source;
        group.bench_with_input(BenchmarkId::from_parameter(n), &src, |b, src| {
            b.iter(|| black_box(parse(black_box(src))));
        });
    }
    group.finish();
}

fn apply_body(c: &mut Criterion) {
    let mut group = c.benchmark_group("apply_body");
    for n in SIZES {
        let file = generated(n, TODO_LINE, "TODO");
        let doc = parse(&file.source);
        // Reuses one outline; see `body_edit_reuses_only_the_other_outline`.
        let edits = replace(&file.source, file.body_word, BODY_REPLACEMENT);
        group.bench_function(BenchmarkId::from_parameter(n), |b| {
            b.iter(|| black_box(black_box(&doc).apply(black_box(&edits))));
        });
    }
    group.finish();
}

fn apply_todo_one(c: &mut Criterion) {
    let mut group = c.benchmark_group("apply_todo_one");
    for n in SIZES {
        let file = generated(n, NEXT_LINE, "NEXT");
        let doc = parse(&file.source);
        let start = NEXT_LINE
            .find(" NEXT")
            .expect("NEXT sits on the keyword line");
        let range = start..start + " NEXT".len();
        assert_eq!(&file.source[range.start..range.end], " NEXT");
        // A text hop, then a recarve of the one NEXT heading; see
        // `todo_edit_recarves_a_heading_outside_the_text_unit`.
        let edits = replace(&file.source, range, "");
        group.bench_function(BenchmarkId::from_parameter(n), |b| {
            b.iter(|| black_box(black_box(&doc).apply(black_box(&edits))));
        });
    }
    group.finish();
}

fn apply_todo_all(c: &mut Criterion) {
    let mut group = c.benchmark_group("apply_todo_all");
    for n in SIZES {
        let file = generated(n, TODO_LINE, "TODO");
        let doc = parse(&file.source);
        // The vocabulary TODO, not the `#+TODO:` key and not a heading.
        let start = "#+TODO: ".len();
        let range = start..start + "TODO".len();
        assert_eq!(&file.source[range.start..range.end], "TODO");
        // The same two hops, but every heading word leaves the vocabulary,
        // so every heading is recarved.
        let edits = replace(&file.source, range, "TASK");
        group.bench_function(BenchmarkId::from_parameter(n), |b| {
            b.iter(|| black_box(black_box(&doc).apply(black_box(&edits))));
        });
    }
    group.finish();
}

fn cold_after(c: &mut Criterion) {
    let mut group = c.benchmark_group("cold_after");
    for n in SIZES {
        let file = generated(n, TODO_LINE, "TODO");
        let doc = parse(&file.source);
        let edits = replace(&file.source, file.body_word, BODY_REPLACEMENT);
        let after = doc.apply(&edits).source().to_owned();
        group.bench_with_input(BenchmarkId::from_parameter(n), &after, |b, src| {
            b.iter(|| black_box(parse(black_box(src))));
        });
    }
    group.finish();
}

criterion_group!(
    baseline,
    cold,
    apply_body,
    apply_todo_one,
    apply_todo_all,
    cold_after
);
criterion_main!(baseline);
