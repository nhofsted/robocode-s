// SPDX-License-Identifier: EPL-2.0

//! Country flags of bots whose author chose none (spec §16.7), as LiteRumble shows them: the
//! RoboWiki page `RoboRumble/Country_Flags` maps each author's "main" package to an ISO
//! 3166-1 alpha-3 code. The daily sync reads it into [`FLAGS_KEY`]; publish takes a bot's flag
//! from its id's first segment (`jk` for `jk.mega.DrussGT`), matched exactly, as LiteRumble
//! does.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// RoboWiki's flags page, raw.
pub const FLAGS_PAGE: &str =
    "https://robowiki.net/w/index.php?title=RoboRumble/Country_Flags&action=raw";
/// The packages' flags, as the last trusted read of the page had them.
pub const FLAGS_KEY: &str = "state/flags.json";
/// The page itself.
pub const MAX_FLAGS_BYTES: u64 = 1 << 20;

/// The flag codes are ISO 3166-1 alpha-2 in lower case, as the owners' countries, plus these
/// two of the page: `WIKI` (wiki bots; LiteRumble shows the United Nations flag) and `MAN`
/// (anarcho-capitalist).
pub const WIKI_FLAG: &str = "un";
pub const ANCAP_FLAG: &str = "ancap";

/// `state/flags.json`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FlagMap {
    /// Package → flag code.
    pub packages: BTreeMap<String, String>,
    /// When the sync read the page (unix s).
    pub at: u64,
}

impl FlagMap {
    /// The flag of a bot by its id's package, if the page lists it.
    pub fn of(&self, bot_id: &str) -> Option<&str> {
        let package = bot_id.split('.').next()?;
        self.packages.get(package).map(String::as_str)
    }
}

/// What the page holds: the flags of its packages and the lines that are none (an unknown
/// code, no comma). `BLANK` (an author who wants no flag) is neither.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ParsedFlags {
    pub packages: BTreeMap<String, String>,
    pub unknown: Vec<String>,
}

/// Parses the page in `action=raw` form: the lines between `<pre>` and `</pre>`, each
/// `<package>,<code>`. A package listed twice keeps its first line.
pub fn parse_flags(text: &str) -> ParsedFlags {
    let mut out = ParsedFlags::default();
    let Some(start) = text.find("<pre>") else {
        return out;
    };
    let body = &text[start + 5..];
    let body = &body[..body.find("</pre>").unwrap_or(body.len())];
    for line in body.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let Some((package, code)) = line.split_once(',') else {
            out.unknown.push(line.to_string());
            continue;
        };
        let (package, code) = (package.trim(), code.trim());
        if code.eq_ignore_ascii_case("BLANK") {
            continue;
        }
        match flag_code(code) {
            Some(flag) if !package.is_empty() && !package.contains(['.', ' ']) => {
                out.packages
                    .entry(package.to_string())
                    .or_insert_with(|| flag.to_string());
            }
            _ => out.unknown.push(line.to_string()),
        }
    }
    out
}

/// The flag code for a code of the page: an ISO 3166-1 alpha-3 code, `WIKI` or `MAN`.
pub fn flag_code(code: &str) -> Option<&'static str> {
    let code = code.to_ascii_uppercase();
    match code.as_str() {
        "WIKI" => Some(WIKI_FLAG),
        "MAN" => Some(ANCAP_FLAG),
        _ => ALPHA3
            .binary_search_by(|(a3, _)| a3.cmp(&code.as_str()))
            .ok()
            .map(|i| ALPHA3[i].1),
    }
}

/// ISO 3166-1 alpha-3 → alpha-2 (lower case), sorted (from Debian's iso-codes).
#[rustfmt::skip]
const ALPHA3: &[(&str, &str)] = &[
    ("ABW", "aw"), ("AFG", "af"), ("AGO", "ao"), ("AIA", "ai"), ("ALA", "ax"), ("ALB", "al"),
    ("AND", "ad"), ("ARE", "ae"), ("ARG", "ar"), ("ARM", "am"), ("ASM", "as"), ("ATA", "aq"),
    ("ATF", "tf"), ("ATG", "ag"), ("AUS", "au"), ("AUT", "at"), ("AZE", "az"), ("BDI", "bi"),
    ("BEL", "be"), ("BEN", "bj"), ("BES", "bq"), ("BFA", "bf"), ("BGD", "bd"), ("BGR", "bg"),
    ("BHR", "bh"), ("BHS", "bs"), ("BIH", "ba"), ("BLM", "bl"), ("BLR", "by"), ("BLZ", "bz"),
    ("BMU", "bm"), ("BOL", "bo"), ("BRA", "br"), ("BRB", "bb"), ("BRN", "bn"), ("BTN", "bt"),
    ("BVT", "bv"), ("BWA", "bw"), ("CAF", "cf"), ("CAN", "ca"), ("CCK", "cc"), ("CHE", "ch"),
    ("CHL", "cl"), ("CHN", "cn"), ("CIV", "ci"), ("CMR", "cm"), ("COD", "cd"), ("COG", "cg"),
    ("COK", "ck"), ("COL", "co"), ("COM", "km"), ("CPV", "cv"), ("CRI", "cr"), ("CUB", "cu"),
    ("CUW", "cw"), ("CXR", "cx"), ("CYM", "ky"), ("CYP", "cy"), ("CZE", "cz"), ("DEU", "de"),
    ("DJI", "dj"), ("DMA", "dm"), ("DNK", "dk"), ("DOM", "do"), ("DZA", "dz"), ("ECU", "ec"),
    ("EGY", "eg"), ("ERI", "er"), ("ESH", "eh"), ("ESP", "es"), ("EST", "ee"), ("ETH", "et"),
    ("FIN", "fi"), ("FJI", "fj"), ("FLK", "fk"), ("FRA", "fr"), ("FRO", "fo"), ("FSM", "fm"),
    ("GAB", "ga"), ("GBR", "gb"), ("GEO", "ge"), ("GGY", "gg"), ("GHA", "gh"), ("GIB", "gi"),
    ("GIN", "gn"), ("GLP", "gp"), ("GMB", "gm"), ("GNB", "gw"), ("GNQ", "gq"), ("GRC", "gr"),
    ("GRD", "gd"), ("GRL", "gl"), ("GTM", "gt"), ("GUF", "gf"), ("GUM", "gu"), ("GUY", "gy"),
    ("HKG", "hk"), ("HMD", "hm"), ("HND", "hn"), ("HRV", "hr"), ("HTI", "ht"), ("HUN", "hu"),
    ("IDN", "id"), ("IMN", "im"), ("IND", "in"), ("IOT", "io"), ("IRL", "ie"), ("IRN", "ir"),
    ("IRQ", "iq"), ("ISL", "is"), ("ISR", "il"), ("ITA", "it"), ("JAM", "jm"), ("JEY", "je"),
    ("JOR", "jo"), ("JPN", "jp"), ("KAZ", "kz"), ("KEN", "ke"), ("KGZ", "kg"), ("KHM", "kh"),
    ("KIR", "ki"), ("KNA", "kn"), ("KOR", "kr"), ("KWT", "kw"), ("LAO", "la"), ("LBN", "lb"),
    ("LBR", "lr"), ("LBY", "ly"), ("LCA", "lc"), ("LIE", "li"), ("LKA", "lk"), ("LSO", "ls"),
    ("LTU", "lt"), ("LUX", "lu"), ("LVA", "lv"), ("MAC", "mo"), ("MAF", "mf"), ("MAR", "ma"),
    ("MCO", "mc"), ("MDA", "md"), ("MDG", "mg"), ("MDV", "mv"), ("MEX", "mx"), ("MHL", "mh"),
    ("MKD", "mk"), ("MLI", "ml"), ("MLT", "mt"), ("MMR", "mm"), ("MNE", "me"), ("MNG", "mn"),
    ("MNP", "mp"), ("MOZ", "mz"), ("MRT", "mr"), ("MSR", "ms"), ("MTQ", "mq"), ("MUS", "mu"),
    ("MWI", "mw"), ("MYS", "my"), ("MYT", "yt"), ("NAM", "na"), ("NCL", "nc"), ("NER", "ne"),
    ("NFK", "nf"), ("NGA", "ng"), ("NIC", "ni"), ("NIU", "nu"), ("NLD", "nl"), ("NOR", "no"),
    ("NPL", "np"), ("NRU", "nr"), ("NZL", "nz"), ("OMN", "om"), ("PAK", "pk"), ("PAN", "pa"),
    ("PCN", "pn"), ("PER", "pe"), ("PHL", "ph"), ("PLW", "pw"), ("PNG", "pg"), ("POL", "pl"),
    ("PRI", "pr"), ("PRK", "kp"), ("PRT", "pt"), ("PRY", "py"), ("PSE", "ps"), ("PYF", "pf"),
    ("QAT", "qa"), ("REU", "re"), ("ROU", "ro"), ("RUS", "ru"), ("RWA", "rw"), ("SAU", "sa"),
    ("SDN", "sd"), ("SEN", "sn"), ("SGP", "sg"), ("SGS", "gs"), ("SHN", "sh"), ("SJM", "sj"),
    ("SLB", "sb"), ("SLE", "sl"), ("SLV", "sv"), ("SMR", "sm"), ("SOM", "so"), ("SPM", "pm"),
    ("SRB", "rs"), ("SSD", "ss"), ("STP", "st"), ("SUR", "sr"), ("SVK", "sk"), ("SVN", "si"),
    ("SWE", "se"), ("SWZ", "sz"), ("SXM", "sx"), ("SYC", "sc"), ("SYR", "sy"), ("TCA", "tc"),
    ("TCD", "td"), ("TGO", "tg"), ("THA", "th"), ("TJK", "tj"), ("TKL", "tk"), ("TKM", "tm"),
    ("TLS", "tl"), ("TON", "to"), ("TTO", "tt"), ("TUN", "tn"), ("TUR", "tr"), ("TUV", "tv"),
    ("TWN", "tw"), ("TZA", "tz"), ("UGA", "ug"), ("UKR", "ua"), ("UMI", "um"), ("URY", "uy"),
    ("USA", "us"), ("UZB", "uz"), ("VAT", "va"), ("VCT", "vc"), ("VEN", "ve"), ("VGB", "vg"),
    ("VIR", "vi"), ("VNM", "vn"), ("VUT", "vu"), ("WLF", "wf"), ("WSM", "ws"), ("YEM", "ye"),
    ("ZAF", "za"), ("ZMB", "zm"), ("ZWE", "zw"),
];

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: &str = "In the rankings tables bots are represented with a flag.\n\
        <code>\nABW AFG\n</code>\n\
        Please add your packages in alphabetical order:\n<pre>\n\
        aaa,CHN\n\
        jk,ZAF\n\
        axeBots, BRA \n\
        pez,MAN\n\
        sample,WIKI\n\
        nobody,BLANK\n\
        weird,XXX\n\
        no comma\n\
        jk,USA\n\
        \n</pre>\nafter,USA\n";

    #[test]
    fn parses_the_pre_block() {
        let parsed = parse_flags(PAGE);
        let expect: BTreeMap<String, String> = [
            ("aaa", "cn"),
            ("axeBots", "br"),
            ("jk", "za"),
            ("pez", "ancap"),
            ("sample", "un"),
        ]
        .into_iter()
        .map(|(p, f)| (p.to_string(), f.to_string()))
        .collect();
        assert_eq!(parsed.packages, expect);
        assert_eq!(parsed.unknown, vec!["weird,XXX", "no comma"]);
        assert!(parse_flags("aaa,CHN").packages.is_empty());
    }

    #[test]
    fn table_is_sorted_and_complete() {
        assert!(ALPHA3.windows(2).all(|w| w[0].0 < w[1].0));
        assert_eq!(ALPHA3.len(), 249);
        assert_eq!(flag_code("GBR"), Some("gb"));
        assert_eq!(flag_code("nld"), Some("nl"));
        assert_eq!(flag_code("ANK"), None);
    }

    #[test]
    fn a_bot_takes_its_first_segment() {
        let flags = FlagMap {
            packages: [("jk".to_string(), "za".to_string())].into(),
            at: 0,
        };
        assert_eq!(flags.of("jk.mega.DrussGT"), Some("za"));
        assert_eq!(flags.of("jkx.Bot"), None);
        assert_eq!(flags.of("Jk.Bot"), None);
    }
}
