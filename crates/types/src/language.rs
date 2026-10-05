//! The languages Nebo speaks — the app's own list
//! (`app/src/lib/i18n/languages.ts`): the code each is kept under, and its
//! name as a prompt says it.

/// Every language Nebo speaks: (code, name in English and in itself).
pub const LANGUAGES: &[(&str, &str)] = &[
    ("en", "English"),
    ("de", "German (Deutsch)"),
    ("es", "Spanish (Español)"),
    ("fr", "French (Français)"),
    ("it", "Italian (Italiano)"),
    ("pt", "Portuguese (Português)"),
    ("pt-BR", "Brazilian Portuguese (Português do Brasil)"),
    ("nl", "Dutch (Nederlands)"),
    ("sv", "Swedish (Svenska)"),
    ("pl", "Polish (Polski)"),
    ("tr", "Turkish (Türkçe)"),
    ("ru", "Russian (Русский)"),
    ("uk", "Ukrainian (Українська)"),
    ("ar", "Arabic (العربية)"),
    ("he", "Hebrew (עברית)"),
    ("hi", "Hindi (हिन्दी)"),
    ("bn", "Bengali (বাংলা)"),
    ("th", "Thai (ไทย)"),
    ("vi", "Vietnamese (Tiếng Việt)"),
    ("id", "Indonesian (Bahasa Indonesia)"),
    ("ms", "Malay (Bahasa Melayu)"),
    ("ja", "Japanese (日本語)"),
    ("ko", "Korean (한국어)"),
    ("zh-CN", "Simplified Chinese (简体中文)"),
    ("zh-TW", "Traditional Chinese (繁體中文)"),
];

/// The language Nebo speaks that a BCP-47 tag reads as: "es-MX" → "es",
/// "pt-BR" → "pt-BR", "pt-PT" → "pt", "zh-Hant-TW" → "zh-TW", "zh" →
/// "zh-CN". None for a language Nebo does not speak.
pub fn app_language(tag: &str) -> Option<&'static str> {
    let tag = tag.trim().replace('_', "-").to_ascii_lowercase();
    if let Some((code, _)) = LANGUAGES.iter().find(|(c, _)| c.eq_ignore_ascii_case(&tag)) {
        return Some(code);
    }
    let mut parts = tag.split('-');
    let base = parts.next().filter(|b| !b.is_empty())?;
    let rest: Vec<&str> = parts.collect();
    match base {
        "zh" if rest
            .iter()
            .any(|p| matches!(*p, "hant" | "tw" | "hk" | "mo")) =>
        {
            Some("zh-TW")
        }
        "zh" => Some("zh-CN"),
        "pt" if rest.contains(&"br") => Some("pt-BR"),
        _ => LANGUAGES.iter().map(|(c, _)| *c).find(|c| *c == base),
    }
}

/// The name a prompt gives the language kept under `code`; English for a
/// code Nebo does not speak.
pub fn display_name(code: &str) -> &'static str {
    LANGUAGES
        .iter()
        .find(|(c, _)| *c == code)
        .map_or("English", |(_, name)| name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_device_tag_reads_as_the_language_nebo_speaks() {
        for (tag, want) in [
            ("es", Some("es")),
            ("es-MX", Some("es")),
            ("en-US", Some("en")),
            ("pt-BR", Some("pt-BR")),
            ("pt_br", Some("pt-BR")),
            ("pt-PT", Some("pt")),
            ("zh-Hant-TW", Some("zh-TW")),
            ("zh-HK", Some("zh-TW")),
            ("zh-Hans-CN", Some("zh-CN")),
            ("zh", Some("zh-CN")),
            ("sv-SE", Some("sv")),
            ("fi", None),
            ("", None),
        ] {
            assert_eq!(app_language(tag), want, "{tag}");
        }
    }

    #[test]
    fn every_language_has_its_name() {
        assert_eq!(LANGUAGES.len(), 25);
        assert_eq!(display_name("sv"), "Swedish (Svenska)");
        assert_eq!(display_name("es"), "Spanish (Español)");
        assert_eq!(display_name("xx"), "English");
    }
}
