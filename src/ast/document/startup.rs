//! `#+STARTUP:` words, read as Org's `org-startup-options` reads them.
//!
//! Every live `#+STARTUP:` line counts, in document order; its value is
//! split on whitespace, each word is matched ASCII caseless, and a later
//! word of a group replaces an earlier one. Words Org does not know are
//! kept in order, so an editor can show them as written.

use super::source_slice;
use crate::{Document, TextRange, TextSize};

/// The option a `#+STARTUP:` word sets. The words of one group are
/// alternatives; [`StartupGroup::words`] lists them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub enum StartupGroup {
    /// How the outline is folded when the file opens.
    Visibility,
    /// Indented view.
    Indent,
    /// Numbered headings.
    Numbering,
    /// Leading stars hidden or shown.
    Stars,
    /// Odd levels only.
    OddLevels,
    /// Tables aligned on open.
    Align,
    /// Tables shrunk on open.
    Shrink,
    /// Links shown by description or literally.
    Links,
    /// Images and link previews shown inline.
    Previews,
    /// LaTeX fragments previewed.
    LatexPreview,
    /// Custom time format.
    CustomTime,
    /// What is recorded when a task is done.
    LogDone,
    /// A note when the clock stops.
    LogNoteClockOut,
    /// What is recorded when a repeating task is done.
    LogRepeat,
    /// Log lines into a drawer.
    LogDrawer,
    /// Newest log entry first.
    LogStatesReversed,
    /// What is recorded when a scheduled date changes.
    LogReschedule,
    /// What is recorded when a deadline changes.
    LogRedeadline,
    /// What is recorded when an entry is refiled.
    LogRefile,
    /// Footnotes defined inline.
    FootnoteInline,
    /// Footnotes kept local to their section.
    FootnoteLocal,
    /// How footnote labels are made.
    FootnoteLabels,
    /// Footnotes renumbered and sorted.
    FootnoteAdjust,
    /// Unit system for constants.
    Constants,
    /// The user's persistent tags ignored.
    PersonalTags,
    /// Blocks folded on open.
    HideBlocks,
    /// Drawers folded on open.
    HideDrawers,
    /// Beamer mode.
    Beamer,
    /// Entities drawn as characters or as written.
    Entities,
}

const GROUPS: &[(StartupGroup, &[&str])] = &[
    (
        StartupGroup::Visibility,
        &[
            "overview",
            "content",
            "showall",
            "show2levels",
            "show3levels",
            "show4levels",
            "show5levels",
            "showeverything",
            "fold",
            "nofold",
        ],
    ),
    (StartupGroup::Indent, &["indent", "noindent"]),
    (StartupGroup::Numbering, &["num", "nonum"]),
    (StartupGroup::Stars, &["hidestars", "showstars"]),
    (StartupGroup::OddLevels, &["odd", "oddeven"]),
    (StartupGroup::Align, &["align", "noalign"]),
    (StartupGroup::Shrink, &["shrink"]),
    (StartupGroup::Links, &["descriptivelinks", "literallinks"]),
    (
        StartupGroup::Previews,
        &[
            "inlineimages",
            "noinlineimages",
            "linkpreviews",
            "nolinkpreviews",
        ],
    ),
    (
        StartupGroup::LatexPreview,
        &["latexpreview", "nolatexpreview"],
    ),
    (StartupGroup::CustomTime, &["customtime"]),
    (
        StartupGroup::LogDone,
        &["logdone", "lognotedone", "nologdone"],
    ),
    (
        StartupGroup::LogNoteClockOut,
        &["lognoteclock-out", "nolognoteclock-out"],
    ),
    (
        StartupGroup::LogRepeat,
        &["logrepeat", "lognoterepeat", "nologrepeat"],
    ),
    (StartupGroup::LogDrawer, &["logdrawer", "nologdrawer"]),
    (
        StartupGroup::LogStatesReversed,
        &["logstatesreversed", "nologstatesreversed"],
    ),
    (
        StartupGroup::LogReschedule,
        &["logreschedule", "lognotereschedule", "nologreschedule"],
    ),
    (
        StartupGroup::LogRedeadline,
        &["logredeadline", "lognoteredeadline", "nologredeadline"],
    ),
    (
        StartupGroup::LogRefile,
        &["logrefile", "lognoterefile", "nologrefile"],
    ),
    (StartupGroup::FootnoteInline, &["fninline", "nofninline"]),
    (StartupGroup::FootnoteLocal, &["fnlocal"]),
    (
        StartupGroup::FootnoteLabels,
        &["fnauto", "fnprompt", "fnconfirm", "fnplain", "fnanon"],
    ),
    (StartupGroup::FootnoteAdjust, &["fnadjust", "nofnadjust"]),
    (StartupGroup::Constants, &["constcgs", "constSI"]),
    (StartupGroup::PersonalTags, &["noptag"]),
    (StartupGroup::HideBlocks, &["hideblocks", "nohideblocks"]),
    (StartupGroup::HideDrawers, &["hidedrawers", "nohidedrawers"]),
    (StartupGroup::Beamer, &["beamer"]),
    (StartupGroup::Entities, &["entitiespretty", "entitiesplain"]),
];

impl StartupGroup {
    /// The words that set this option, as Org spells them.
    #[must_use]
    pub fn words(self) -> &'static [&'static str] {
        GROUPS
            .iter()
            .find(|(group, _)| *group == self)
            .map_or(&[], |(_, words)| words)
    }

    /// The group `word` belongs to and its canonical spelling, matched
    /// ASCII caseless; `None` for a word Org does not know.
    #[must_use]
    pub fn of(word: &str) -> Option<(Self, &'static str)> {
        GROUPS.iter().find_map(|(group, words)| {
            words
                .iter()
                .find(|w| w.eq_ignore_ascii_case(word))
                .map(|w| (*group, *w))
        })
    }
}

/// How the outline is folded when the file opens
/// ([`StartupGroup::Visibility`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum StartupVisibility {
    /// Top-level headings only (`overview`, `fold`).
    Overview,
    /// Every heading, no text (`content`).
    Content,
    /// Everything but drawers (`showall`, `nofold`).
    ShowAll,
    /// Headings down to this level, no text (`show2levels` to `show5levels`).
    ShowLevels(u8),
    /// Everything, drawers included (`showeverything`).
    ShowEverything,
}

impl StartupVisibility {
    fn from_word(word: &str) -> Option<Self> {
        Some(match word {
            "overview" | "fold" => Self::Overview,
            "content" => Self::Content,
            "showall" | "nofold" => Self::ShowAll,
            "show2levels" => Self::ShowLevels(2),
            "show3levels" => Self::ShowLevels(3),
            "show4levels" => Self::ShowLevels(4),
            "show5levels" => Self::ShowLevels(5),
            "showeverything" => Self::ShowEverything,
            _ => return None,
        })
    }
}

/// One `#+STARTUP:` word Org knows.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StartupWord {
    group: StartupGroup,
    word: &'static str,
    range: TextRange,
}

impl StartupWord {
    /// The option the word sets.
    #[must_use]
    pub fn group(&self) -> StartupGroup {
        self.group
    }

    /// The word in Org's spelling, whatever case it was written in.
    #[must_use]
    pub fn word(&self) -> &'static str {
        self.word
    }

    /// The word's range in the source.
    #[must_use]
    pub fn range(&self) -> TextRange {
        self.range
    }
}

/// A document's `#+STARTUP:` words, in order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Startup {
    words: Vec<StartupWord>,
    unknown: Vec<(String, TextRange)>,
}

impl Startup {
    /// Every word Org knows, in document order, repeats included.
    #[must_use]
    pub fn words(&self) -> &[StartupWord] {
        &self.words
    }

    /// Words Org does not know, as written, in document order.
    pub fn unknown(&self) -> impl Iterator<Item = (&str, TextRange)> + '_ {
        self.unknown
            .iter()
            .map(|(word, range)| (word.as_str(), *range))
    }

    /// The word that sets `group`: the last one written.
    #[must_use]
    pub fn get(&self, group: StartupGroup) -> Option<&StartupWord> {
        self.words.iter().rev().find(|w| w.group == group)
    }

    /// The folding the file asks for when it opens.
    #[must_use]
    pub fn visibility(&self) -> Option<StartupVisibility> {
        self.get(StartupGroup::Visibility)
            .and_then(|w| StartupVisibility::from_word(w.word))
    }

    /// Whether the document has no `#+STARTUP:` words.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.words.is_empty() && self.unknown.is_empty()
    }
}

impl Document {
    /// The `#+STARTUP:` words of every live line, in document order.
    #[must_use]
    pub fn startup(&self) -> Startup {
        let mut out = Startup::default();
        for kw in self.keywords().filter(|kw| kw.key_eq("startup")) {
            let range = kw.value().content_range();
            let value = source_slice(self.source(), range);
            let mut rest = value;
            while let Some(start) = rest.find(|c: char| !c.is_whitespace()) {
                let tail = &rest[start..];
                let len = tail.find(char::is_whitespace).unwrap_or(tail.len());
                let word = &tail[..len];
                let offset = value.len() - rest.len() + start;
                let begin = range.start() + text_size(offset);
                let at = TextRange::at(begin, text_size(len));
                match StartupGroup::of(word) {
                    Some((group, canonical)) => out.words.push(StartupWord {
                        group,
                        word: canonical,
                        range: at,
                    }),
                    None => out.unknown.push((word.to_owned(), at)),
                }
                rest = &tail[len..];
            }
        }
        out
    }
}

/// An offset inside a keyword value, which the source's length bounds.
fn text_size(n: usize) -> TextSize {
    TextSize::try_from(n).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn startup(src: &str) -> Startup {
        crate::parse(src).startup()
    }

    #[test]
    fn a_later_word_of_a_group_wins_across_lines() {
        let s = startup("#+STARTUP: overview logdone\n* A\n#+startup: CONTENT\n");
        assert_eq!(s.visibility(), Some(StartupVisibility::Content));
        assert_eq!(
            s.get(StartupGroup::LogDone).map(StartupWord::word),
            Some("logdone")
        );
        assert_eq!(s.words().len(), 3);
    }

    #[test]
    fn aliases_and_levels_read_as_org_reads_them() {
        assert_eq!(
            startup("#+STARTUP: fold\n").visibility(),
            Some(StartupVisibility::Overview)
        );
        assert_eq!(
            startup("#+STARTUP: nofold\n").visibility(),
            Some(StartupVisibility::ShowAll)
        );
        assert_eq!(
            startup("#+STARTUP: show3levels\n").visibility(),
            Some(StartupVisibility::ShowLevels(3))
        );
        assert_eq!(startup("* A\n").visibility(), None);
    }

    #[test]
    fn unknown_words_are_kept_with_their_ranges() {
        let src = "#+STARTUP:  hidestars  frobnicate constsi\n";
        let s = startup(src);
        let unknown: Vec<_> = s.unknown().collect();
        assert_eq!(unknown.len(), 1);
        let (word, range) = unknown[0];
        assert_eq!(word, "frobnicate");
        assert_eq!(&src[range], "frobnicate");
        assert_eq!(
            s.get(StartupGroup::Constants).map(StartupWord::word),
            Some("constSI")
        );
        let stars = s.get(StartupGroup::Stars).map(StartupWord::range);
        assert_eq!(stars.map(|r| &src[r]), Some("hidestars"));
    }

    #[test]
    fn a_startup_line_in_a_block_does_not_count() {
        let s = startup("#+begin_example\n#+STARTUP: overview\n#+end_example\n");
        assert!(s.is_empty());
    }

    #[test]
    fn every_word_belongs_to_one_group() {
        for (group, words) in GROUPS {
            for word in *words {
                assert_eq!(StartupGroup::of(word).map(|(g, _)| g), Some(*group));
            }
        }
    }
}
