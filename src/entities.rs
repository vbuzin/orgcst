//! Org entities: definitions, the builtin name table, and [`entity_form`].
//!
//! Recognition and export both look names up through
//! [`crate::ParseConfig::entity_get`] / [`crate::ParseConfig::is_entity_name`],
//! which consult the user map first and then this table, so user entities can
//! shadow builtins.
//!
//! The builtin set is a hand-maintained snapshot of Emacs `org-entities`
//! (names and export forms); it is not regenerated from Emacs. Keep the table
//! sorted by `name` (Rust byte order) so `builtin_entity_get` can
//! binary-search; tests pin sort order, uniqueness and a few known names.
//!
//! Field order matches the `org-entities` tuple: name, LaTeX, LaTeX-math flag,
//! HTML, ASCII, Latin-1, UTF-8. The `\_` space family (1..=20 spaces) is part
//! of the same sorted table.

use std::borrow::Cow;

/// One Org entity definition, builtin or user-supplied.
///
/// Field order matches the Org entity tuple: name, LaTeX, LaTeX-math flag,
/// HTML, ASCII, Latin-1, UTF-8.
///
/// Builtin rows are `const`-constructed and borrow `'static` strings; user
/// rows own theirs. The fields are `Cow` so one type serves both, and they
/// are private: the accessors hand back `&str` whichever source the
/// definition came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntityDef {
    /// Entity name without the leading backslash (e.g. `"alpha"`, `"_ "`).
    name: Cow<'static, str>,
    /// LaTeX replacement.
    latex: Cow<'static, str>,
    /// Whether the LaTeX form should be in math mode.
    latex_math: bool,
    /// HTML replacement.
    html: Cow<'static, str>,
    /// Plain ASCII replacement.
    ascii: Cow<'static, str>,
    /// Latin-1 replacement.
    latin1: Cow<'static, str>,
    /// UTF-8 replacement.
    utf8: Cow<'static, str>,
}

/// Compact `const` constructor for [`BUILTIN_ENTITIES`] rows.
#[inline]
const fn entity(
    name: &'static str,
    latex: &'static str,
    latex_math: bool,
    html: &'static str,
    ascii: &'static str,
    latin1: &'static str,
    utf8: &'static str,
) -> EntityDef {
    EntityDef {
        name: Cow::Borrowed(name),
        latex: Cow::Borrowed(latex),
        latex_math,
        html: Cow::Borrowed(html),
        ascii: Cow::Borrowed(ascii),
        latin1: Cow::Borrowed(latin1),
        utf8: Cow::Borrowed(utf8),
    }
}

impl EntityDef {
    /// Build an entity definition.
    ///
    /// `&'static str` arguments stay borrowed; `String` arguments are owned, so
    /// a runtime-built definition costs one allocation per field and a literal
    /// costs none.
    ///
    /// Does not validate the name. [`crate::ParseConfig::with_user_entities`] only
    /// registers names that pass [`Self::is_user_name_safe`]; others are
    /// dropped at the config boundary.
    pub fn new(
        name: impl Into<Cow<'static, str>>,
        latex: impl Into<Cow<'static, str>>,
        latex_math: bool,
        html: impl Into<Cow<'static, str>>,
        ascii: impl Into<Cow<'static, str>>,
        latin1: impl Into<Cow<'static, str>>,
        utf8: impl Into<Cow<'static, str>>,
    ) -> Self {
        Self {
            name: name.into(),
            latex: latex.into(),
            latex_math,
            html: html.into(),
            ascii: ascii.into(),
            latin1: latin1.into(),
            utf8: utf8.into(),
        }
    }

    /// Entity name without the leading backslash (e.g. `"alpha"`, `"_ "`).
    #[inline]
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// LaTeX replacement.
    #[inline]
    #[must_use]
    pub fn latex(&self) -> &str {
        &self.latex
    }

    /// Whether the LaTeX form should be in math mode.
    #[inline]
    #[must_use]
    pub fn latex_math(&self) -> bool {
        self.latex_math
    }

    /// HTML replacement.
    #[inline]
    #[must_use]
    pub fn html(&self) -> &str {
        &self.html
    }

    /// Plain ASCII replacement.
    #[inline]
    #[must_use]
    pub fn ascii(&self) -> &str {
        &self.ascii
    }

    /// Latin-1 replacement.
    #[inline]
    #[must_use]
    pub fn latin1(&self) -> &str {
        &self.latin1
    }

    /// UTF-8 replacement.
    #[inline]
    #[must_use]
    pub fn utf8(&self) -> &str {
        &self.utf8
    }

    /// Org `org-entities-user` safe name: ASCII letter, then letters or digits.
    ///
    /// Mirrors `org-entities--user-safe-p`'s letter-then-alnum name pattern.
    /// Whitespace entity names (`"_ "`, ...) and other non-alnum forms are
    /// not user-registerable; they exist only in the builtin table.
    #[inline]
    #[must_use]
    pub fn is_user_name_safe(name: &str) -> bool {
        let mut chars = name.chars();
        match chars.next() {
            Some(c) if c.is_ascii_alphabetic() => chars.all(|c| c.is_ascii_alphanumeric()),
            _ => false,
        }
    }
}

/// Sorted builtin entity table.
///
/// Rows are constructed with [`entity`]. Keep sorted by `name` (byte order).
#[rustfmt::skip]
pub(crate) static BUILTIN_ENTITIES: &[EntityDef] = &[
    entity("AA", "\\AA{}", false, "&Aring;", "A", "\u{c5}", "\u{c5}"),
    entity("AElig", "\\AE{}", false, "&AElig;", "AE", "\u{c6}", "\u{c6}"),
    entity("Aacute", "\\'{A}", false, "&Aacute;", "A", "\u{c1}", "\u{c1}"),
    entity("Acirc", "\\^{A}", false, "&Acirc;", "A", "\u{c2}", "\u{c2}"),
    entity("Agrave", "\\`{A}", false, "&Agrave;", "A", "\u{c0}", "\u{c0}"),
    entity("Alpha", "A", false, "&Alpha;", "Alpha", "Alpha", "\u{391}"),
    entity("Amacr", "\\={A}", false, "&Amacr;", "A", "\u{c3}", "\u{c3}"),
    entity("Aring", "\\AA{}", false, "&Aring;", "A", "\u{c5}", "\u{c5}"),
    entity("Atilde", "\\~{A}", false, "&Atilde;", "A", "\u{c3}", "\u{c3}"),
    entity("Auml", "\\\"{A}", false, "&Auml;", "Ae", "\u{c4}", "\u{c4}"),
    entity("Beta", "B", false, "&Beta;", "Beta", "Beta", "\u{392}"),
    entity("Ccedil", "\\c{C}", false, "&Ccedil;", "C", "\u{c7}", "\u{c7}"),
    entity("Chi", "X", false, "&Chi;", "Chi", "Chi", "\u{3a7}"),
    entity("Dagger", "\\textdaggerdbl{}", false, "&Dagger;", "[doubledagger]", "[doubledagger]", "\u{2021}"),
    entity("Delta", "\\Delta", true, "&Delta;", "Delta", "Delta", "\u{394}"),
    entity("Diamond", "\\diamondsuit", true, "&diamond;", "[diamond]", "[diamond]", "\u{25c6}"),
    entity("Downarrow", "\\Downarrow", true, "&dArr;", "[dbldownarrow]", "[dbldownarrow]", "\u{21d3}"),
    entity("ETH", "\\DH{}", false, "&ETH;", "D", "\u{d0}", "\u{d0}"),
    entity("EUR", "\\texteuro{}", false, "&euro;", "EUR", "EUR", "\u{20ac}"),
    entity("Eacute", "\\'{E}", false, "&Eacute;", "E", "\u{c9}", "\u{c9}"),
    entity("Ecirc", "\\^{E}", false, "&Ecirc;", "E", "\u{ca}", "\u{ca}"),
    entity("Egrave", "\\`{E}", false, "&Egrave;", "E", "\u{c8}", "\u{c8}"),
    entity("Epsilon", "E", false, "&Epsilon;", "Epsilon", "Epsilon", "\u{395}"),
    entity("Eta", "H", false, "&Eta;", "Eta", "Eta", "\u{397}"),
    entity("Euml", "\\\"{E}", false, "&Euml;", "E", "\u{cb}", "\u{cb}"),
    entity("Gamma", "\\Gamma", true, "&Gamma;", "Gamma", "Gamma", "\u{393}"),
    entity("Gg", "\\ggg", true, "&Gg;", ">>>", ">>>", "\u{22d9}"),
    entity("Iacute", "\\'{I}", false, "&Iacute;", "I", "\u{cd}", "\u{cd}"),
    entity("Icirc", "\\^{I}", false, "&Icirc;", "I", "\u{ce}", "\u{ce}"),
    entity("Idot", "\\.{I}", false, "&idot;", "I", "\u{130}", "\u{130}"),
    entity("Igrave", "\\`{I}", false, "&Igrave;", "I", "\u{cc}", "\u{cc}"),
    entity("Iota", "I", false, "&Iota;", "Iota", "Iota", "\u{399}"),
    entity("Iuml", "\\\"{I}", false, "&Iuml;", "I", "\u{cf}", "\u{cf}"),
    entity("Kappa", "K", false, "&Kappa;", "Kappa", "Kappa", "\u{39a}"),
    entity("Lambda", "\\Lambda", true, "&Lambda;", "Lambda", "Lambda", "\u{39b}"),
    entity("Leftarrow", "\\Leftarrow", true, "&lArr;", "<=", "<=", "\u{21d0}"),
    entity("Leftrightarrow", "\\Leftrightarrow", true, "&hArr;", "<=>", "<=>", "\u{21d4}"),
    entity("Ll", "\\lll", true, "&Ll;", "<<<", "<<<", "\u{22d8}"),
    entity("Mu", "M", false, "&Mu;", "Mu", "Mu", "\u{39c}"),
    entity("Ntilde", "\\~{N}", false, "&Ntilde;", "N", "\u{d1}", "\u{d1}"),
    entity("Nu", "N", false, "&Nu;", "Nu", "Nu", "\u{39d}"),
    entity("OElig", "\\OE{}", false, "&OElig;", "OE", "OE", "\u{152}"),
    entity("Oacute", "\\'{O}", false, "&Oacute;", "O", "\u{d3}", "\u{d3}"),
    entity("Ocirc", "\\^{O}", false, "&Ocirc;", "O", "\u{d4}", "\u{d4}"),
    entity("Ograve", "\\`{O}", false, "&Ograve;", "O", "\u{d2}", "\u{d2}"),
    entity("Omega", "\\Omega", true, "&Omega;", "Omega", "Omega", "\u{3a9}"),
    entity("Omicron", "O", false, "&Omicron;", "Omicron", "Omicron", "\u{39f}"),
    entity("Oslash", "\\O", false, "&Oslash;", "O", "\u{d8}", "\u{d8}"),
    entity("Otilde", "\\~{O}", false, "&Otilde;", "O", "\u{d5}", "\u{d5}"),
    entity("Ouml", "\\\"{O}", false, "&Ouml;", "Oe", "\u{d6}", "\u{d6}"),
    entity("P", "\\P{}", false, "&para;", "paragraph", "\u{b6}", "\u{b6}"),
    entity("Phi", "\\Phi", true, "&Phi;", "Phi", "Phi", "\u{3a6}"),
    entity("Pi", "\\Pi", true, "&Pi;", "Pi", "Pi", "\u{3a0}"),
    entity("Pr", "\\Pr", true, "Pr", "Pr", "Pr", "Pr"),
    entity("Prime", "\\prime{}\\prime", true, "&Prime;", "''", "''", "\u{2033}"),
    entity("Psi", "\\Psi", true, "&Psi;", "Psi", "Psi", "\u{3a8}"),
    entity("Rho", "P", false, "&Rho;", "Rho", "Rho", "\u{3a1}"),
    entity("Rightarrow", "\\Rightarrow", true, "&rArr;", "=>", "=>", "\u{21d2}"),
    entity("S", "\\S", false, "&sect;", "section", "\u{a7}", "\u{a7}"),
    entity("Scaron", "\\v{S}", false, "&Scaron;", "S", "S", "\u{160}"),
    entity("Sigma", "\\Sigma", true, "&Sigma;", "Sigma", "Sigma", "\u{3a3}"),
    entity("THORN", "\\TH{}", false, "&THORN;", "TH", "\u{de}", "\u{de}"),
    entity("Tau", "T", false, "&Tau;", "Tau", "Tau", "\u{3a4}"),
    entity("Theta", "\\Theta", true, "&Theta;", "Theta", "Theta", "\u{398}"),
    entity("USD", "\\$", false, "$", "$", "$", "$"),
    entity("Uacute", "\\'{U}", false, "&Uacute;", "U", "\u{da}", "\u{da}"),
    entity("Ucirc", "\\^{U}", false, "&Ucirc;", "U", "\u{db}", "\u{db}"),
    entity("Ugrave", "\\`{U}", false, "&Ugrave;", "U", "\u{d9}", "\u{d9}"),
    entity("Uparrow", "\\Uparrow", true, "&uArr;", "[dbluparrow]", "[dbluparrow]", "\u{21d1}"),
    entity("Upsilon", "\\Upsilon", true, "&Upsilon;", "Upsilon", "Upsilon", "\u{3a5}"),
    entity("Uuml", "\\\"{U}", false, "&Uuml;", "Ue", "\u{dc}", "\u{dc}"),
    entity("Xi", "\\Xi", true, "&Xi;", "Xi", "Xi", "\u{39e}"),
    entity("Yacute", "\\'{Y}", false, "&Yacute;", "Y", "\u{dd}", "\u{dd}"),
    entity("Yuml", "\\\"{Y}", false, "&Yuml;", "Y", "Y", "\u{178}"),
    entity("Zeta", "Z", false, "&Zeta;", "Zeta", "Zeta", "\u{396}"),
    entity("_ ", "\\hspace*{0.5em}", false, "&ensp;", " ", " ", "\u{2002}"),
    entity("_  ", "\\hspace*{1.0em}", false, "&ensp;&ensp;", "  ", "  ", "\u{2002}\u{2002}"),
    entity("_   ", "\\hspace*{1.5em}", false, "&ensp;&ensp;&ensp;", "   ", "   ", "\u{2002}\u{2002}\u{2002}"),
    entity("_    ", "\\hspace*{2.0em}", false, "&ensp;&ensp;&ensp;&ensp;", "    ", "    ", "\u{2002}\u{2002}\u{2002}\u{2002}"),
    entity("_     ", "\\hspace*{2.5em}", false, "&ensp;&ensp;&ensp;&ensp;&ensp;", "     ", "     ", "\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}"),
    entity("_      ", "\\hspace*{3.0em}", false, "&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;", "      ", "      ", "\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}"),
    entity("_       ", "\\hspace*{3.5em}", false, "&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;", "       ", "       ", "\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}"),
    entity("_        ", "\\hspace*{4.0em}", false, "&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;", "        ", "        ", "\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}"),
    entity("_         ", "\\hspace*{4.5em}", false, "&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;", "         ", "         ", "\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}"),
    entity("_          ", "\\hspace*{5.0em}", false, "&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;", "          ", "          ", "\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}"),
    entity("_           ", "\\hspace*{5.5em}", false, "&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;", "           ", "           ", "\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}"),
    entity("_            ", "\\hspace*{6.0em}", false, "&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;", "            ", "            ", "\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}"),
    entity("_             ", "\\hspace*{6.5em}", false, "&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;", "             ", "             ", "\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}"),
    entity("_              ", "\\hspace*{7.0em}", false, "&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;", "              ", "              ", "\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}"),
    entity("_               ", "\\hspace*{7.5em}", false, "&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;", "               ", "               ", "\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}"),
    entity("_                ", "\\hspace*{8.0em}", false, "&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;", "                ", "                ", "\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}"),
    entity("_                 ", "\\hspace*{8.5em}", false, "&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;", "                 ", "                 ", "\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}"),
    entity("_                  ", "\\hspace*{9.0em}", false, "&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;", "                  ", "                  ", "\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}"),
    entity("_                   ", "\\hspace*{9.5em}", false, "&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;", "                   ", "                   ", "\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}"),
    entity("_                    ", "\\hspace*{10.0em}", false, "&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;&ensp;", "                    ", "                    ", "\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}"),
    entity("aacute", "\\'{a}", false, "&aacute;", "a", "\u{e1}", "\u{e1}"),
    entity("acirc", "\\^{a}", false, "&acirc;", "a", "\u{e2}", "\u{e2}"),
    entity("acute", "\\textasciiacute{}", false, "&acute;", "'", "\u{b4}", "\u{b4}"),
    entity("acutex", "\\acute x", true, "&acute;x", "'x", "'x", "\u{1d465}\u{301}"),
    entity("aelig", "\\ae{}", false, "&aelig;", "ae", "\u{e6}", "\u{e6}"),
    entity("agrave", "\\`{a}", false, "&agrave;", "a", "\u{e0}", "\u{e0}"),
    entity("alefsym", "\\aleph", true, "&alefsym;", "aleph", "aleph", "\u{2135}"),
    entity("aleph", "\\aleph", true, "&aleph;", "aleph", "aleph", "\u{2135}"),
    entity("alpha", "\\alpha", true, "&alpha;", "alpha", "alpha", "\u{3b1}"),
    entity("amacr", "\\={a}", false, "&amacr;", "a", "\u{e3}", "\u{e3}"),
    entity("amp", "\\&", false, "&amp;", "&", "&", "&"),
    entity("ang", "\\angle", true, "&ang;", "[angle]", "[angle]", "\u{2220}"),
    entity("angle", "\\angle", true, "&ang;", "[angle]", "[angle]", "\u{2220}"),
    entity("approx", "\\approx", true, "&asymp;", "[almost equal to]", "[almost equal to]", "\u{2248}"),
    entity("arccos", "\\arccos", true, "arccos", "arccos", "arccos", "arccos"),
    entity("arcsin", "\\arcsin", true, "arcsin", "arcsin", "arcsin", "arcsin"),
    entity("arctan", "\\arctan", true, "arctan", "arctan", "arctan", "arctan"),
    entity("arg", "\\arg", true, "arg", "arg", "arg", "arg"),
    entity("aring", "\\aa{}", false, "&aring;", "a", "\u{e5}", "\u{e5}"),
    entity("asciicirc", "\\textasciicircum{}", false, "^", "^", "^", "^"),
    entity("ast", "\\ast", true, "&lowast;", "*", "*", "*"),
    entity("asymp", "\\asymp", true, "&asymp;", "[almost equal to]", "[almost equal to]", "\u{2248}"),
    entity("atilde", "\\~{a}", false, "&atilde;", "a", "\u{e3}", "\u{e3}"),
    entity("auml", "\\\"{a}", false, "&auml;", "ae", "\u{e4}", "\u{e4}"),
    entity("bdquo", "\\quotedblbase{}", false, "&bdquo;", "\"", "\"", "\u{201e}"),
    entity("because", "\\because", true, "&because;", "[because]", "[because]", "\u{2235}"),
    entity("beta", "\\beta", true, "&beta;", "beta", "beta", "\u{3b2}"),
    entity("beth", "\\beth", true, "&beth;", "beth", "beth", "\u{5d1}"),
    entity("blacksmile", "\\ddot\\smile", true, "&#9787;", ":-)", ":-)", "\u{263b}"),
    entity("brvbar", "\\textbrokenbar{}", false, "&brvbar;", "|", "\u{a6}", "\u{a6}"),
    entity("bull", "\\textbullet{}", false, "&bull;", "*", "*", "\u{2022}"),
    entity("bullet", "\\textbullet{}", false, "&bull;", "*", "*", "\u{2022}"),
    entity("cap", "\\cap", true, "&cap;", "[intersection]", "[intersection]", "\u{2229}"),
    entity("ccedil", "\\c{c}", false, "&ccedil;", "c", "\u{e7}", "\u{e7}"),
    entity("cdot", "\\cdot", true, "&sdot;", "[dot]", "[dot]", "\u{22c5}"),
    entity("cdots", "\\cdots{}", true, "&ctdot;", "...", "...", "\u{22ef}"),
    entity("cedil", "\\c{}", false, "&cedil;", "[cedilla]", "\u{b8}", "\u{b8}"),
    entity("cent", "\\textcent{}", false, "&cent;", "cent", "\u{a2}", "\u{a2}"),
    entity("check", "\\checkmark", true, "&checkmark;", "[checkmark]", "[checkmark]", "\u{2713}"),
    entity("checkmark", "\\checkmark", true, "&check;", "[checkmark]", "[checkmark]", "\u{2713}"),
    entity("chi", "\\chi", true, "&chi;", "chi", "chi", "\u{3c7}"),
    entity("circ", "\\^{}", false, "&circ;", "^", "^", "\u{2218}"),
    entity("clubs", "\\clubsuit", true, "&clubs;", "[clubs]", "[clubs]", "\u{2663}"),
    entity("clubsuit", "\\clubsuit", true, "&clubs;", "[clubs]", "[clubs]", "\u{2663}"),
    entity("colon", "\\colon", true, ":", ":", ":", ":"),
    entity("cong", "\\cong", true, "&cong;", "[approx. equal to]", "[approx. equal to]", "\u{2245}"),
    entity("copy", "\\textcopyright{}", false, "&copy;", "(c)", "\u{a9}", "\u{a9}"),
    entity("cos", "\\cos", true, "cos", "cos", "cos", "cos"),
    entity("cosh", "\\cosh", true, "cosh", "cosh", "cosh", "cosh"),
    entity("cot", "\\cot", true, "cot", "cot", "cot", "cot"),
    entity("coth", "\\coth", true, "coth", "coth", "coth", "coth"),
    entity("crarr", "\\hookleftarrow", true, "&crarr;", "<-'", "<-'", "\u{21b5}"),
    entity("csc", "\\csc", true, "csc", "csc", "csc", "csc"),
    entity("cup", "\\cup", true, "&cup;", "[union]", "[union]", "\u{222a}"),
    entity("curren", "\\textcurrency{}", false, "&curren;", "curr.", "\u{a4}", "\u{a4}"),
    entity("dArr", "\\Downarrow", true, "&dArr;", "[dbldownarrow]", "[dbldownarrow]", "\u{21d3}"),
    entity("dag", "\\dag{}", false, "&dagger;", "[dagger]", "[dagger]", "\u{2020}"),
    entity("dagger", "\\textdagger{}", false, "&dagger;", "[dagger]", "[dagger]", "\u{2020}"),
    entity("dalet", "\\daleth", true, "&daleth;", "dalet", "dalet", "\u{5d3}"),
    entity("darr", "\\downarrow", true, "&darr;", "[downarrow]", "[downarrow]", "\u{2193}"),
    entity("ddag", "\\ddag{}", false, "&Dagger;", "[doubledagger]", "[doubledagger]", "\u{2021}"),
    entity("deg", "\\textdegree{}", false, "&deg;", "degree", "\u{b0}", "\u{b0}"),
    entity("delta", "\\delta", true, "&delta;", "delta", "delta", "\u{3b4}"),
    entity("det", "\\det", true, "det", "det", "det", "det"),
    entity("diamond", "\\diamondsuit", true, "&diamond;", "[diamond]", "[diamond]", "\u{25c6}"),
    entity("diamondsuit", "\\diamondsuit", true, "&diams;", "[diamonds]", "[diamonds]", "\u{25c6}"),
    entity("diams", "\\diamondsuit", true, "&diams;", "[diamonds]", "[diamonds]", "\u{25c6}"),
    entity("dim", "\\dim", true, "dim", "dim", "dim", "dim"),
    entity("div", "\\textdiv{}", false, "&divide;", "/", "\u{f7}", "\u{f7}"),
    entity("dollar", "\\$", false, "$", "$", "$", "$"),
    entity("dots", "\\dots{}", false, "&hellip;", "...", "...", "\u{2026}"),
    entity("downarrow", "\\downarrow", true, "&darr;", "[downarrow]", "[downarrow]", "\u{2193}"),
    entity("eacute", "\\'{e}", false, "&eacute;", "e", "\u{e9}", "\u{e9}"),
    entity("ecirc", "\\^{e}", false, "&ecirc;", "e", "\u{ea}", "\u{ea}"),
    entity("egrave", "\\`{e}", false, "&egrave;", "e", "\u{e8}", "\u{e8}"),
    entity("ell", "\\ell", true, "&ell;", "ell", "ell", "\u{2113}"),
    entity("empty", "\\emptyset", true, "&empty;", "[empty set]", "[empty set]", "\u{2205}"),
    entity("emptyset", "\\emptyset", true, "&empty;", "[empty set]", "[empty set]", "\u{2205}"),
    entity("emsp", "\\hspace*{1em}", false, "&emsp;", " ", " ", "\u{2003}"),
    entity("ensp", "\\hspace*{.5em}", false, "&ensp;", " ", " ", "\u{2002}"),
    entity("epsilon", "\\epsilon", true, "&epsilon;", "epsilon", "epsilon", "\u{3b5}"),
    entity("equal", "=", false, "=", "=", "=", "="),
    entity("equiv", "\\equiv", true, "&equiv;", "[identical to]", "[identical to]", "\u{2261}"),
    entity("eta", "\\eta", true, "&eta;", "eta", "eta", "\u{3b7}"),
    entity("eth", "\\dh{}", false, "&eth;", "dh", "\u{f0}", "\u{f0}"),
    entity("euml", "\\\"{e}", false, "&euml;", "e", "\u{eb}", "\u{eb}"),
    entity("euro", "\\texteuro{}", false, "&euro;", "EUR", "EUR", "\u{20ac}"),
    entity("exist", "\\exists", true, "&exist;", "[there exists]", "[there exists]", "\u{2203}"),
    entity("exists", "\\exists", true, "&exist;", "[there exists]", "[there exists]", "\u{2203}"),
    entity("exp", "\\exp", true, "exp", "exp", "exp", "exp"),
    entity("fnof", "\\textit{f}", false, "&fnof;", "f", "f", "\u{192}"),
    entity("forall", "\\forall", true, "&forall;", "[for all]", "[for all]", "\u{2200}"),
    entity("frac12", "\\textonehalf{}", false, "&frac12;", "1/2", "\u{bd}", "\u{bd}"),
    entity("frac14", "\\textonequarter{}", false, "&frac14;", "1/4", "\u{bc}", "\u{bc}"),
    entity("frac34", "\\textthreequarters{}", false, "&frac34;", "3/4", "\u{be}", "\u{be}"),
    entity("frasl", "/", false, "&frasl;", "/", "/", "\u{2044}"),
    entity("frown", "\\frown", true, "&frown;", "[Cap product]", "[cap product]", "\u{2322}"),
    entity("frowny", "\\ddot\\frown", true, "&#9785;", ":-(", ":-(", "\u{2639}"),
    entity("gamma", "\\gamma", true, "&gamma;", "gamma", "gamma", "\u{3b3}"),
    entity("gcd", "\\gcd", true, "gcd", "gcd", "gcd", "gcd"),
    entity("ge", "\\ge", true, "&ge;", ">=", ">=", "\u{2265}"),
    entity("geq", "\\ge", true, "&ge;", ">=", ">=", "\u{2265}"),
    entity("gets", "\\gets", true, "&larr;", "<-", "<-", "\u{2190}"),
    entity("gg", "\\gg", true, "&Gt;", ">>", ">>", "\u{226b}"),
    entity("ggg", "\\ggg", true, "&Gg;", ">>>", ">>>", "\u{22d9}"),
    entity("gimel", "\\gimel", true, "&gimel;", "gimel", "gimel", "\u{2137}"),
    entity("gt", "\\textgreater{}", false, "&gt;", ">", ">", ">"),
    entity("hArr", "\\Leftrightarrow", true, "&hArr;", "<=>", "<=>", "\u{21d4}"),
    entity("harr", "\\leftrightarrow", true, "&harr;", "<->", "<->", "\u{2194}"),
    entity("hbar", "\\hbar", true, "&hbar;", "hbar", "hbar", "\u{210f}"),
    entity("hearts", "\\heartsuit", true, "&hearts;", "[hearts]", "[hearts]", "\u{2665}"),
    entity("heartsuit", "\\heartsuit", true, "&heartsuit;", "[hearts]", "[hearts]", "\u{2665}"),
    entity("hellip", "\\dots{}", false, "&hellip;", "...", "...", "\u{2026}"),
    entity("hom", "\\hom", true, "hom", "hom", "hom", "hom"),
    entity("hookleftarrow", "\\hookleftarrow", true, "&crarr;", "<-'", "<-'", "\u{21b5}"),
    entity("iacute", "\\'{i}", false, "&iacute;", "i", "\u{ed}", "\u{ed}"),
    entity("icirc", "\\^{i}", false, "&icirc;", "i", "\u{ee}", "\u{ee}"),
    entity("iexcl", "!`", false, "&iexcl;", "!", "\u{a1}", "\u{a1}"),
    entity("igrave", "\\`{i}", false, "&igrave;", "i", "\u{ec}", "\u{ec}"),
    entity("image", "\\Im", true, "&image;", "I", "I", "\u{2111}"),
    entity("imath", "\\imath", true, "&imath;", "[dotless i]", "dotless i", "\u{131}"),
    entity("in", "\\in", true, "&isin;", "[element of]", "[element of]", "\u{2208}"),
    entity("inf", "\\inf", true, "inf", "inf", "inf", "inf"),
    entity("infin", "\\infty", true, "&infin;", "[infinity]", "[infinity]", "\u{221e}"),
    entity("infty", "\\infty", true, "&infin;", "[infinity]", "[infinity]", "\u{221e}"),
    entity("inodot", "\\i", false, "&inodot;", "i", "\u{131}", "\u{131}"),
    entity("int", "\\int", true, "&int;", "[integral]", "[integral]", "\u{222b}"),
    entity("iota", "\\iota", true, "&iota;", "iota", "iota", "\u{3b9}"),
    entity("iquest", "?`", false, "&iquest;", "?", "\u{bf}", "\u{bf}"),
    entity("isin", "\\in", true, "&isin;", "[element of]", "[element of]", "\u{2208}"),
    entity("iuml", "\\\"{i}", false, "&iuml;", "i", "\u{ef}", "\u{ef}"),
    entity("jmath", "\\jmath", true, "&jmath;", "[dotless j]", "dotless j", "\u{237}"),
    entity("kappa", "\\kappa", true, "&kappa;", "kappa", "kappa", "\u{3ba}"),
    entity("ker", "\\ker", true, "ker", "ker", "ker", "ker"),
    entity("lArr", "\\Leftarrow", true, "&lArr;", "<=", "<=", "\u{21d0}"),
    entity("lambda", "\\lambda", true, "&lambda;", "lambda", "lambda", "\u{3bb}"),
    entity("land", "\\land", true, "&and;", "[logical and]", "[logical and]", "\u{2227}"),
    entity("lang", "\\langle", true, "&lang;", "<", "<", "\u{27e8}"),
    entity("langle", "\\langle", true, "&lang;", "<", "<", "\u{27e8}"),
    entity("laquo", "\\guillemotleft{}", false, "&laquo;", "<<", "\u{ab}", "\u{ab}"),
    entity("larr", "\\leftarrow", true, "&larr;", "<-", "<-", "\u{2190}"),
    entity("lceil", "\\lceil", true, "&lceil;", "[left ceiling]", "[left ceiling]", "\u{2308}"),
    entity("ldquo", "\\textquotedblleft{}", false, "&ldquo;", "\"", "\"", "\u{201c}"),
    entity("le", "\\le", true, "&le;", "<=", "<=", "\u{2264}"),
    entity("leftarrow", "\\leftarrow", true, "&larr;", "<-", "<-", "\u{2190}"),
    entity("leftrightarrow", "\\leftrightarrow", true, "&harr;", "<->", "<->", "\u{2194}"),
    entity("leq", "\\le", true, "&le;", "<=", "<=", "\u{2264}"),
    entity("lesseqgtr", "\\lesseqgtr", true, "&lesseqgtr;", "[less than or equal or greater than or equal]", "[less than or equal or greater than or equal]", "\u{22da}"),
    entity("lessgtr", "\\lessgtr", true, "&lessgtr;", "[less than or greater than]", "[less than or greater than]", "\u{2276}"),
    entity("lfloor", "\\lfloor", true, "&lfloor;", "[left floor]", "[left floor]", "\u{230a}"),
    entity("lg", "\\lg", true, "lg", "lg", "lg", "lg"),
    entity("lim", "\\lim", true, "lim", "lim", "lim", "lim"),
    entity("liminf", "\\liminf", true, "liminf", "liminf", "liminf", "liminf"),
    entity("limsup", "\\limsup", true, "limsup", "limsup", "limsup", "limsup"),
    entity("ll", "\\ll", true, "&Lt;", "<<", "<<", "\u{226a}"),
    entity("lll", "\\lll", true, "&Ll;", "<<<", "<<<", "\u{22d8}"),
    entity("ln", "\\ln", true, "ln", "ln", "ln", "ln"),
    entity("log", "\\log", true, "log", "log", "log", "log"),
    entity("lor", "\\lor", true, "&or;", "[logical or]", "[logical or]", "\u{2228}"),
    entity("lowast", "\\ast", true, "&lowast;", "*", "*", "\u{2217}"),
    entity("loz", "\\lozenge", true, "&loz;", "[lozenge]", "[lozenge]", "\u{29eb}"),
    entity("lrm", "", false, "&lrm;", "", "", "\u{200e}"),
    entity("lsaquo", "\\guilsinglleft{}", false, "&lsaquo;", "<", "<", "\u{2039}"),
    entity("lsquo", "\\textquoteleft{}", false, "&lsquo;", "`", "`", "\u{2018}"),
    entity("lt", "\\textless{}", false, "&lt;", "<", "<", "<"),
    entity("macr", "\\textasciimacron{}", false, "&macr;", "[macron]", "\u{af}", "\u{af}"),
    entity("max", "\\max", true, "max", "max", "max", "max"),
    entity("mdash", "---", false, "&mdash;", "--", "--", "\u{2014}"),
    entity("mho", "\\mho", true, "&mho;", "mho", "mho", "\u{2127}"),
    entity("micro", "\\textmu{}", false, "&micro;", "micro", "\u{b5}", "\u{b5}"),
    entity("middot", "\\textperiodcentered{}", false, "&middot;", ".", "\u{b7}", "\u{b7}"),
    entity("min", "\\min", true, "min", "min", "min", "min"),
    entity("minus", "-", true, "&minus;", "-", "-", "\u{2212}"),
    entity("mu", "\\mu", true, "&mu;", "mu", "mu", "\u{3bc}"),
    entity("nabla", "\\nabla", true, "&nabla;", "[nabla]", "[nabla]", "\u{2207}"),
    entity("nbsp", "~", false, "&nbsp;", " ", "\u{a0}", "\u{a0}"),
    entity("ndash", "--", false, "&ndash;", "-", "-", "\u{2013}"),
    entity("ne", "\\ne", true, "&ne;", "[not equal to]", "[not equal to]", "\u{2260}"),
    entity("neg", "\\neg{}", true, "&not;", "[angled dash]", "\u{ac}", "\u{ac}"),
    entity("neq", "\\neq", true, "&ne;", "[not equal to]", "[not equal to]", "\u{2260}"),
    entity("nexist", "\\nexists", true, "&exist;", "[there does not exists]", "[there does not  exists]", "\u{2204}"),
    entity("nexists", "\\nexists", true, "&exist;", "[there does not exists]", "[there does not  exists]", "\u{2204}"),
    entity("ni", "\\ni", true, "&ni;", "[contains as member]", "[contains as member]", "\u{220b}"),
    entity("not", "\\textlnot{}", false, "&not;", "[angled dash]", "\u{ac}", "\u{ac}"),
    entity("notin", "\\notin", true, "&notin;", "[not an element of]", "[not an element of]", "\u{2209}"),
    entity("nsub", "\\not\\subset", true, "&nsub;", "[not a subset of]", "[not a subset of", "\u{2284}"),
    entity("nsup", "\\not\\supset", true, "&nsup;", "[not a superset of]", "[not a superset of]", "\u{2285}"),
    entity("ntilde", "\\~{n}", false, "&ntilde;", "n", "\u{f1}", "\u{f1}"),
    entity("nu", "\\nu", true, "&nu;", "nu", "nu", "\u{3bd}"),
    entity("oacute", "\\'{o}", false, "&oacute;", "o", "\u{f3}", "\u{f3}"),
    entity("ocirc", "\\^{o}", false, "&ocirc;", "o", "\u{f4}", "\u{f4}"),
    entity("odot", "\\odot", true, "o", "[circled dot]", "[circled dot]", "\u{298}"),
    entity("oelig", "\\oe{}", false, "&oelig;", "oe", "oe", "\u{153}"),
    entity("ograve", "\\`{o}", false, "&ograve;", "o", "\u{f2}", "\u{f2}"),
    entity("oline", "\\overline{~}", true, "&oline;", "[overline]", "\u{af}", "\u{203e}"),
    entity("omega", "\\omega", true, "&omega;", "omega", "omega", "\u{3c9}"),
    entity("omicron", "\\textit{o}", false, "&omicron;", "omicron", "omicron", "\u{3bf}"),
    entity("oplus", "\\oplus", true, "&oplus;", "[circled plus]", "[circled plus]", "\u{2295}"),
    entity("ordf", "\\textordfeminine{}", false, "&ordf;", "_a_", "\u{aa}", "\u{aa}"),
    entity("ordm", "\\textordmasculine{}", false, "&ordm;", "_o_", "\u{ba}", "\u{ba}"),
    entity("oslash", "\\o{}", false, "&oslash;", "o", "\u{f8}", "\u{f8}"),
    entity("otilde", "\\~{o}", false, "&otilde;", "o", "\u{f5}", "\u{f5}"),
    entity("otimes", "\\otimes", true, "&otimes;", "[circled times]", "[circled times]", "\u{2297}"),
    entity("ouml", "\\\"{o}", false, "&ouml;", "oe", "\u{f6}", "\u{f6}"),
    entity("para", "\\P{}", false, "&para;", "paragraph", "\u{b6}", "\u{b6}"),
    entity("parallel", "\\parallel", true, "&parallel;", "||", "||", "\u{2225}"),
    entity("partial", "\\partial", true, "&part;", "[partial differential]", "[partial differential]", "\u{2202}"),
    entity("permil", "\\textperthousand{}", false, "&permil;", "per thousand", "per thousand", "\u{2030}"),
    entity("perp", "\\perp", true, "&perp;", "[up tack]", "[up tack]", "\u{22a5}"),
    entity("phi", "\\phi", true, "&phi;", "phi", "phi", "\u{278}"),
    entity("pi", "\\pi", true, "&pi;", "pi", "pi", "\u{3c0}"),
    entity("piv", "\\varpi", true, "&piv;", "omega-pi", "omega-pi", "\u{3d6}"),
    entity("plus", "+", false, "+", "+", "+", "+"),
    entity("plusmn", "\\textpm{}", false, "&plusmn;", "+-", "\u{b1}", "\u{b1}"),
    entity("pm", "\\textpm{}", false, "&plusmn;", "+-", "\u{b1}", "\u{b1}"),
    entity("pound", "\\pounds{}", false, "&pound;", "pound", "\u{a3}", "\u{a3}"),
    entity("prec", "\\prec", true, "&pr;", "[precedes]", "[precedes]", "\u{227a}"),
    entity("preccurlyeq", "\\preccurlyeq", true, "&prcue;", "[precedes or equal]", "[precedes or equal]", "\u{227c}"),
    entity("preceq", "\\preceq", true, "&prcue;", "[precedes or equal]", "[precedes or equal]", "\u{227c}"),
    entity("prime", "\\prime", true, "&prime;", "'", "'", "\u{2032}"),
    entity("prod", "\\prod", true, "&prod;", "[product]", "[n-ary product]", "\u{220f}"),
    entity("prop", "\\propto", true, "&prop;", "[proportional to]", "[proportional to]", "\u{221d}"),
    entity("propto", "\\propto", true, "&prop;", "[proportional to]", "[proportional to]", "\u{221d}"),
    entity("psi", "\\psi", true, "&psi;", "psi", "psi", "\u{3c8}"),
    entity("quot", "\\textquotedbl{}", false, "&quot;", "\"", "\"", "\""),
    entity("rArr", "\\Rightarrow", true, "&rArr;", "=>", "=>", "\u{21d2}"),
    entity("radic", "\\sqrt{\\,}", true, "&radic;", "[square root]", "[square root]", "\u{221a}"),
    entity("rang", "\\rangle", true, "&rang;", ">", ">", "\u{27e9}"),
    entity("rangle", "\\rangle", true, "&rang;", ">", ">", "\u{27e9}"),
    entity("raquo", "\\guillemotright{}", false, "&raquo;", ">>", "\u{bb}", "\u{bb}"),
    entity("rarr", "\\rightarrow", true, "&rarr;", "->", "->", "\u{2192}"),
    entity("rceil", "\\rceil", true, "&rceil;", "[right ceiling]", "[right ceiling]", "\u{2309}"),
    entity("rdquo", "\\textquotedblright{}", false, "&rdquo;", "\"", "\"", "\u{201d}"),
    entity("real", "\\Re", true, "&real;", "R", "R", "\u{211c}"),
    entity("reg", "\\textregistered{}", false, "&reg;", "(r)", "\u{ae}", "\u{ae}"),
    entity("rfloor", "\\rfloor", true, "&rfloor;", "[right floor]", "[right floor]", "\u{230b}"),
    entity("rho", "\\rho", true, "&rho;", "rho", "rho", "\u{3c1}"),
    entity("rightarrow", "\\rightarrow", true, "&rarr;", "->", "->", "\u{2192}"),
    entity("rlm", "", false, "&rlm;", "", "", "\u{200f}"),
    entity("rsaquo", "\\guilsinglright{}", false, "&rsaquo;", ">", ">", "\u{203a}"),
    entity("rsquo", "\\textquoteright{}", false, "&rsquo;", "'", "'", "\u{2019}"),
    entity("sad", "\\ddot\\frown", true, "&#9785;", ":-(", ":-(", "\u{2639}"),
    entity("sbquo", "\\quotesinglbase{}", false, "&sbquo;", ",", ",", "\u{201a}"),
    entity("scaron", "\\v{s}", false, "&scaron;", "s", "s", "\u{161}"),
    entity("sdot", "\\cdot", true, "&sdot;", "[dot]", "[dot]", "\u{22c5}"),
    entity("sec", "\\sec", true, "sec", "sec", "sec", "sec"),
    entity("sect", "\\S", false, "&sect;", "section", "\u{a7}", "\u{a7}"),
    entity("shy", "\\-", false, "&shy;", "", "", ""),
    entity("sigma", "\\sigma", true, "&sigma;", "sigma", "sigma", "\u{3c3}"),
    entity("sigmaf", "\\varsigma", true, "&sigmaf;", "sigmaf", "sigmaf", "\u{3c2}"),
    entity("sim", "\\sim", true, "&sim;", "~", "~", "\u{223c}"),
    entity("simeq", "\\simeq", true, "&cong;", "[approx. equal to]", "[approx. equal to]", "\u{2245}"),
    entity("sin", "\\sin", true, "sin", "sin", "sin", "sin"),
    entity("sinh", "\\sinh", true, "sinh", "sinh", "sinh", "sinh"),
    entity("slash", "/", false, "/", "/", "/", "/"),
    entity("smile", "\\smile", true, "&smile;", "[cup product]", "[cup product]", "\u{2323}"),
    entity("smiley", "\\ddot\\smile", true, "&#9786;", ":-)", ":-)", "\u{263a}"),
    entity("spades", "\\spadesuit", true, "&spades;", "[spades]", "[spades]", "\u{2660}"),
    entity("spadesuit", "\\spadesuit", true, "&spades;", "[spades]", "[spades]", "\u{2660}"),
    entity("star", "\\star", true, "*", "*", "*", "\u{22c6}"),
    entity("sub", "\\subset", true, "&sub;", "[subset of]", "[subset of]", "\u{2282}"),
    entity("sube", "\\subseteq", true, "&sube;", "[subset of or equal to]", "[subset of or equal to]", "\u{2286}"),
    entity("subset", "\\subset", true, "&sub;", "[subset of]", "[subset of]", "\u{2282}"),
    entity("succ", "\\succ", true, "&sc;", "[succeeds]", "[succeeds]", "\u{227b}"),
    entity("succcurlyeq", "\\succcurlyeq", true, "&sccue;", "[succeeds or equal]", "[succeeds or equal]", "\u{227d}"),
    entity("succeq", "\\succeq", true, "&sccue;", "[succeeds or equal]", "[succeeds or equal]", "\u{227d}"),
    entity("sum", "\\sum", true, "&sum;", "[sum]", "[sum]", "\u{2211}"),
    entity("sup", "\\supset", true, "&sup;", "[superset of]", "[superset of]", "\u{2283}"),
    entity("sup1", "\\textonesuperior{}", false, "&sup1;", "^1", "\u{b9}", "\u{b9}"),
    entity("sup2", "\\texttwosuperior{}", false, "&sup2;", "^2", "\u{b2}", "\u{b2}"),
    entity("sup3", "\\textthreesuperior{}", false, "&sup3;", "^3", "\u{b3}", "\u{b3}"),
    entity("supe", "\\supseteq", true, "&supe;", "[superset of or equal to]", "[superset of or equal to]", "\u{2287}"),
    entity("supset", "\\supset", true, "&sup;", "[superset of]", "[superset of]", "\u{2283}"),
    entity("szlig", "\\ss{}", false, "&szlig;", "ss", "\u{df}", "\u{df}"),
    entity("tan", "\\tan", true, "tan", "tan", "tan", "tan"),
    entity("tanh", "\\tanh", true, "tanh", "tanh", "tanh", "tanh"),
    entity("tau", "\\tau", true, "&tau;", "tau", "tau", "\u{3c4}"),
    entity("there4", "\\therefore", true, "&there4;", "[therefore]", "[therefore]", "\u{2234}"),
    entity("therefore", "\\therefore", true, "&there4;", "[therefore]", "[therefore]", "\u{2234}"),
    entity("theta", "\\theta", true, "&theta;", "theta", "theta", "\u{3b8}"),
    entity("thetasym", "\\vartheta", true, "&thetasym;", "theta", "theta", "\u{3d1}"),
    entity("thinsp", "\\hspace*{.2em}", false, "&thinsp;", " ", " ", "\u{2009}"),
    entity("thorn", "\\th{}", false, "&thorn;", "th", "\u{fe}", "\u{fe}"),
    entity("tilde", "\\textasciitilde{}", false, "~", "~", "~", "~"),
    entity("times", "\\texttimes{}", false, "&times;", "*", "\u{d7}", "\u{d7}"),
    entity("to", "\\to", true, "&rarr;", "->", "->", "\u{2192}"),
    entity("trade", "\\texttrademark{}", false, "&trade;", "TM", "TM", "\u{2122}"),
    entity("triangleq", "\\triangleq", true, "&triangleq;", "[defined to]", "[defined to]", "\u{225c}"),
    entity("uArr", "\\Uparrow", true, "&uArr;", "[dbluparrow]", "[dbluparrow]", "\u{21d1}"),
    entity("uacute", "\\'{u}", false, "&uacute;", "u", "\u{fa}", "\u{fa}"),
    entity("uarr", "\\uparrow", true, "&uarr;", "[uparrow]", "[uparrow]", "\u{2191}"),
    entity("ucirc", "\\^{u}", false, "&ucirc;", "u", "\u{fb}", "\u{fb}"),
    entity("ugrave", "\\`{u}", false, "&ugrave;", "u", "\u{f9}", "\u{f9}"),
    entity("uml", "\\textasciidieresis{}", false, "&uml;", "[diaeresis]", "\u{a8}", "\u{a8}"),
    entity("under", "\\_", false, "_", "_", "_", "_"),
    entity("uparrow", "\\uparrow", true, "&uarr;", "[uparrow]", "[uparrow]", "\u{2191}"),
    entity("upsih", "\\Upsilon", true, "&upsih;", "upsilon", "upsilon", "\u{3d2}"),
    entity("upsilon", "\\upsilon", true, "&upsilon;", "upsilon", "upsilon", "\u{3c5}"),
    entity("uuml", "\\\"{u}", false, "&uuml;", "ue", "\u{fc}", "\u{fc}"),
    entity("varepsilon", "\\varepsilon", true, "&epsilon;", "varepsilon", "varepsilon", "\u{3b5}"),
    entity("varphi", "\\varphi", true, "&varphi;", "varphi", "varphi", "\u{3c6}"),
    entity("varpi", "\\varpi", true, "&piv;", "omega-pi", "omega-pi", "\u{3d6}"),
    entity("varsigma", "\\varsigma", true, "&sigmaf;", "varsigma", "varsigma", "\u{3c2}"),
    entity("vartheta", "\\vartheta", true, "&thetasym;", "theta", "theta", "\u{3d1}"),
    entity("vbar", "|", false, "|", "|", "|", "|"),
    entity("vee", "\\vee", true, "&or;", "[logical or]", "[logical or]", "\u{2228}"),
    entity("vert", "\\vert{}", true, "&vert;", "|", "|", "|"),
    entity("wedge", "\\wedge", true, "&and;", "[logical and]", "[logical and]", "\u{2227}"),
    entity("weierp", "\\wp", true, "&weierp;", "P", "P", "\u{2118}"),
    entity("xi", "\\xi", true, "&xi;", "xi", "xi", "\u{3be}"),
    entity("yacute", "\\'{y}", false, "&yacute;", "y", "\u{fd}", "\u{fd}"),
    entity("yen", "\\textyen{}", false, "&yen;", "yen", "\u{a5}", "\u{a5}"),
    entity("yuml", "\\\"{y}", false, "&yuml;", "y", "\u{ff}", "\u{ff}"),
    entity("zeta", "\\zeta", true, "&zeta;", "zeta", "zeta", "\u{3b6}"),
    entity("zwj", "", false, "&zwj;", "", "", "\u{200d}"),
    entity("zwnj", "\\/{}", false, "&zwnj;", "", "", "\u{200c}"),
];

/// Look up a builtin entity by exact name (binary search).
///
/// Recognition goes through [`crate::ParseConfig::entity_get`] instead, so
/// user entities are never bypassed.
#[inline]
pub(crate) fn builtin_entity_get(name: &str) -> Option<&'static EntityDef> {
    BUILTIN_ENTITIES
        .binary_search_by_key(&name, |e| &*e.name)
        .ok()
        .map(|i| &BUILTIN_ENTITIES[i])
}

/// Parsed entity source form (`\name` or `\name{}` or whitespace name).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntityForm<'a> {
    /// Name without leading backslash.
    pub name: &'a str,
    /// True when the form used a `{}` terminator.
    pub use_brackets: bool,
}

/// Split the text of an `ENTITY` node into its name and `{}` terminator.
///
/// Pass the exact node text from the CST. The name is taken as the parser
/// measured it: this function does not check it against the entity table or
/// apply the rules for what may follow an entity.
#[must_use]
pub fn entity_form(text: &str) -> Option<EntityForm<'_>> {
    let rest = text.strip_prefix('\\')?;
    if rest.is_empty() {
        return None;
    }
    // Whitespace entity: _ + spaces (no braces).
    if rest.as_bytes().first() == Some(&b'_') {
        let after = &rest[1..];
        if after.is_empty() || !after.bytes().all(|b| b == b' ') {
            return None;
        }
        return Some(EntityForm {
            name: rest,
            use_brackets: false,
        });
    }
    let (name, use_brackets) = if let Some(name) = rest.strip_suffix("{}") {
        (name, true)
    } else {
        (rest, false)
    };
    if name.is_empty() {
        return None;
    }
    Some(EntityForm { name, use_brackets })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_table_sorted_and_searchable() {
        // Pin the row count and the order so a dropped or duplicated row fails
        // loudly when the table is edited. Update the count when adding or
        // removing entities.
        assert_eq!(BUILTIN_ENTITIES.len(), 411);
        for w in BUILTIN_ENTITIES.windows(2) {
            assert!(
                w[0].name() < w[1].name(),
                "not sorted: {:?} {:?}",
                w[0].name(),
                w[1].name()
            );
        }
        assert!(builtin_entity_get("alpha").is_some());
        assert!(builtin_entity_get("Alpha").is_some());
        assert!(builtin_entity_get("there4").is_some());
        assert!(builtin_entity_get("frac12").is_some());
        assert!(builtin_entity_get("_ ").is_some());
        assert!(builtin_entity_get("alphax").is_none());
        assert!(builtin_entity_get("").is_none());
        let a = builtin_entity_get("alpha").expect("alpha");
        assert_eq!(a.name(), "alpha");
        assert_eq!(a.utf8(), "\u{3b1}");
        assert_eq!(a.html(), "&alpha;");
        assert!(a.latex_math());
    }

    /// Builtin rows borrow, user rows own, and both compare by content through
    /// the one public type.
    #[test]
    fn builtin_rows_borrow_user_rows_own() {
        let builtin = builtin_entity_get("alpha").expect("alpha");
        assert!(
            matches!(builtin.name, Cow::Borrowed(_)),
            "builtin table must not allocate"
        );

        let owned = EntityDef::new(
            "alpha",
            String::from("\\alpha"),
            true,
            "&alpha;",
            "alpha",
            "alpha",
            "\u{3b1}",
        );
        assert!(matches!(owned.latex, Cow::Owned(_)), "String arg owns");
        assert!(
            matches!(owned.name, Cow::Borrowed(_)),
            "literal stays borrowed"
        );
        // Content equality across representations.
        assert_eq!(&owned, builtin);
    }

    #[test]
    fn entity_form_is_green_span_projection() {
        let f = entity_form("\\alpha").expect("form");
        assert_eq!(f.name, "alpha");
        assert!(!f.use_brackets);
        let f = entity_form("\\alpha{}").expect("form");
        assert_eq!(f.name, "alpha");
        assert!(f.use_brackets);
        let f = entity_form("\\_  ").expect("ws");
        assert_eq!(f.name, "_  ");
        assert!(!f.use_brackets);
        // Not a free-form parser: no leading backslash, empty, or garbage.
        assert!(entity_form("alpha").is_none());
        assert!(entity_form("\\").is_none());
        // Callers must pass `ENTITY` node text only. Source running past the
        // name (such as `\\alpha2`) is taken as the name, not re-split.
        let f = entity_form("\\alpha2").expect("text past the entity");
        assert_eq!(f.name, "alpha2");
        assert!(!f.use_brackets);
    }

    #[test]
    fn user_name_safe_predicate() {
        assert!(EntityDef::is_user_name_safe("alpha"));
        assert!(EntityDef::is_user_name_safe("my2ent"));
        assert!(EntityDef::is_user_name_safe("A1"));
        assert!(!EntityDef::is_user_name_safe(""));
        assert!(!EntityDef::is_user_name_safe("1abc"));
        assert!(!EntityDef::is_user_name_safe("_ "));
        assert!(!EntityDef::is_user_name_safe("foo-bar"));
        assert!(!EntityDef::is_user_name_safe("foo_bar"));
    }
}
