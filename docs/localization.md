# Interface language

Choose **Menu > Edit > Preferences…** (Command-comma on macOS, Ctrl-comma elsewhere) **> Interface language** and select **Auto**, **English**, **日本語**, **简体中文**, **繁體中文**, **Русский**, **Български**, **Čeština**, **Português (Brasil)**, **Deutsch**, **Español**, **Français**, **తెలుగు**, **Magyar**, **Українська**, **Italiano** or **العربية**. The change applies immediately and persists between launches. Command ids, document contents and file names are unchanged.

Displayed keyboard shortcuts use the same active interface language. For example, German shows `Strg+K` and `Strg+Umschalt+S` in the toolbar, menus, command palette, tooltips and Keyboard shortcuts dialog; macOS keeps its modifier symbols (`⌘`, `⇧`, `⌃`). Key names have their own `key` catalog context, so the Delete key can read `Entf` while the command remains `Löschen`. A missing key-name entry falls back to the English key name. Actual bindings and machine-facing command ids remain stable.

**Auto** (the default) follows the system language: `LC_ALL`, `LC_MESSAGES` or `LANG`, then the preferred-languages list on macOS. Any Portuguese locale (`pt_BR`, `pt_PT`) uses the Brazilian catalog, any French locale (`fr`, `fr_FR`, `fr_CA`, `fr_BE`) the French one, any German locale (`de`, `de_DE`, `de_AT`, `de_CH`) the German one, and Chinese locales for Taiwan, Hong Kong and Macau (`zh_TW`, `zh_HK`, `zh_MO`, `zh-Hant`) the Traditional Chinese one; other Chinese locales (`zh`, `zh_CN`, `zh_SG`, `zh-Hans`) the Simplified Chinese one. Telugu locales (`te`, `te_IN`) use the Telugu catalog, Bulgarian locales (`bg`, `bg_BG`) the Bulgarian one, and Hungarian locales (`hu`, `hu_HU`) the Hungarian one, and Ukrainian locales (`uk`, `uk_UA`) the Ukrainian one, and Italian locales (`it`, `it_IT`, `it_CH`) the Italian one, and Arabic locales (`ar`, `ar_MA`, `ar_EG`, `ar_SA`, …) the Arabic one. A system language without a catalog shows English. On Windows, **Auto** uses the Windows display languages, in preference order, when no supported locale is selected by those environment variables. Neutral `C`/`POSIX` locale values (including `C.UTF-8`) inherited from a launcher do not override the Windows display language; explicitly named supported locales such as `fr_FR.UTF-8` still do. The query asks Windows directly (`GetUserPreferredUILanguages`, through the `sys-locale` crate) without starting a process, and is cached for the process. If detection fails or the display language has no catalog, PdfCraft shows English.

The control channel exposes the setting through `ui.set`:

```json
{"method":"ui.set","params":{"key":"language","value":"ja"}}
```

The value is `auto` or a language code (`en`, `ja`, `zh-hans`, `zh-hant`, `ru`, `bg`, `cs`, `pt-br`, `de`, `es`, `fr`, `te`, `hu`, `uk`, `it`, `ar`), in any case. `ui.state` reports `language` as written in the preferences (`auto`, `en`, `ja`, `zh-hans`, `zh-hant`, `ru`, `bg`, `cs`, `pt-br`, `de`, `es`, `fr`, `te`, `hu`, `uk`, `it` or `ar`). Unknown values return an error without changing the current setting. Preferences saved before this setting existed follow the system language.

Dialogs, panels, menus, notices and history labels go through the catalog (`tl!`). Japanese, Traditional Chinese, Simplified Chinese, Russian, Bulgarian, German, Spanish, French, Telugu, Hungarian, Ukrainian, Italian, Brazilian Portuguese and Arabic translate them (about 2,000–2,140 entries each); Czech covers the menus so far. Untranslated labels use English. Error details that come from the engine or the operating system are shown as they are, inside a translated frame ("操作「…」に失敗しました: …"). The command palette matches the translated label, the English label and the command id. Vertical Japanese PDF rendering is an existing viewer feature; this change does not add vertical text editing. Per-language coverage, script support and what each language still needs are in [localization-parity.md](localization-parity.md).

The Ukrainian catalog has 2,047 entries, written from the meaning of the English labels using Ukrainian computing vocabulary. Commands use infinitives; fixed-count messages use count-neutral wording. Plural-aware messages use the three [CLDR Ukrainian integer forms](https://www.unicode.org/cldr/charts/48/supplemental/language_plural_rules.html#uk): `1 сторінка`, `2 сторінки`, `5 сторінок`; 11–14 take the third form, while 21 and 22 return to the first and second. Calendar dates use genitive month names (`9 жовтня 2026`), while headings use standalone names (`Жовтень 2026`). Home recommendation cards grow to fit translated text; the toolbar search hint truncates before the shortcut, with the full label available on hover.

Japanese interface text uses BIZ UDPGothic from [craft-fonts](https://github.com/storytold/craft-fonts), an optional build input that every release includes (`CRAFT_FONTS_DIR`; see the README). A build made without it has no Japanese face, so Japanese labels show replacement boxes. Chinese is drawn with the same craft-fonts CJK faces (in Simplified Chinese the `Hans` faces come first, so a line never mixes faces with different baselines), so a character none of them has shows a replacement box (and every Chinese character does in a build without craft-fonts). Russian, Bulgarian, Czech, Brazilian Portuguese, German, Spanish, French, Hungarian and Italian need only the bundled Latin faces (tested in `tests/fonts.rs`). Hungarian uses the same one/other plural rule as English: 1 is singular, and every other count — including 0 — is the other form. After a numeral the noun stays singular (`1 oldal`, `2 oldal`). Telugu is drawn with a craft-fonts `Telu` face (Noto Sans Telugu, kept in the web build too), whose conjuncts egui shapes; a build without one shows Telugu labels as replacement boxes, and the release-pinned craft-fonts input has no Telugu face yet.

The Brazilian Portuguese catalog has 2,000 entries: the ~1,826-entry gist André Cardoso posted in [#288](https://github.com/storytold/pdfcraft/issues/288), the labels added since, and a review of the existing wording. Portuguese, like French, treats 0 and 1 as singular (`1 página`, `2 páginas`).

## Arabic and right-to-left text

The Arabic catalog has 2,137 entries in Modern Standard Arabic with Western digits. Counts use the six [CLDR Arabic forms](https://www.unicode.org/cldr/charts/48/supplemental/language_plural_rules.html#ar) (`0`, `1`, `2`, `3–10`, `11–99`, the rest), and dates use the Arabic names of the Gregorian months.

egui joins the letters of an Arabic word and runs them right to left, but it places the words of a line left to right in the order they are written. So catalog entries are written in logical (typing) order, and `i18n` puts each one into display order when the catalog loads (`bidi::display_rtl`): the line reads right to left, Latin words and numbers inside it read left to right, `{name}` placeholders and keyboard shortcuts stay whole, spaces at either end stay where the code concatenates, and a short caption keeps its colon on the right, beside the widget it captions. Lookups (`tl!`, `tr`, `trn`) therefore return display-order text for Arabic; `i18n::fmt` inserts values as they are. The command search matches a query of several Arabic words word by word.

What Arabic does not have yet:

- **The layout is not mirrored.** Panels, menus, toolbars and alignment are as in the left-to-right languages, with Arabic labels in them.
- **A wrapped paragraph breaks at the wrong end.** A label too long for its line continues on the next line with its *first* words, so wrapped descriptions and tooltips read from the last line up. Single-line labels are right.
- **Text you type into a field** (search, comments, form values) has joined letters but its words run left to right while you edit it. What is stored is correct.
- Labels built by joining translated pieces in code (a list such as `A · B · C`) keep the code's left-to-right order.

These need bidirectional support in the text layout itself, not in the catalog.

Arabic glyphs come from the craft-fonts `Arab` face (Noto Sans Arabic, kept in the web build too). A build without craft-fonts shows Arabic as replacement boxes on the web; on desktop it falls back to one installed font (below). The craft-fonts revision pinned for CI and releases (`.github/workflows/`) has the Arabic face. That revision also holds Noto Sans CJK SC, which `crates/fonts/build.rs` never embeds: `AGENTS.md` §1.1 bars it, so Chinese still has no face of its own.

File names and document titles in Arabic or Hebrew are drawn with a craft-fonts `Arab` face when the build input has one; on desktop, a character no embedded face has is drawn with one font already installed on the machine (`PDFCRAFT_SYSTEM_FONTS=0` turns this off). The tab strip, the recent-files list and Properties show such names in display order; other places (bookmarks, comments, search results) have the glyphs but may order mixed-direction text wrongly.

Japanese, Simplified Chinese, French, German and Brazilian Portuguese coverage tests, following PhotoCraft's complete-catalog approach, check every registered command, All tools label and `tl!("literal")` in the UI source. Simplified Chinese also checks direct `i18n::t` calls and multiline literals, plus the About tabs, contributor name modes, sort options, table headers and model columns. Generated history labels, contributor summaries and diagnostics are tested with user-supplied names and error details. Catalog coverage does not guarantee glyph coverage: the release-pinned craft-fonts input currently has no `Hans` face, and the web build embeds only BIZ UDPGothic Regular. Chinese font delivery remains separate work, as discussed in [#112](https://github.com/storytold/pdfcraft/pull/112) and [#140](https://github.com/storytold/pdfcraft/pull/140).

German labels keep nouns capitalized, including tool names inserted into notices. The certificate viewer uses the `certificate` context for `Subject` (the certificate holder), while document properties keep the plain translation (the topic). Search hints reserve room for the shortcut, and Home cards limit their title and description to the available rows; hovering shows the full text when it is shortened.

## How translations work

PdfCraft uses the same system as PhotoCraft (`crates/ui-egui/src/i18n/`):

- Strings in the code stay in English and serve as lookup keys. At draw time, `tl!("Save")` returns the text in the current language, and falls back to English when there is no entry.
- Each language has a catalog, `crates/ui-egui/src/i18n/<code>.tsv`, with one entry per line: `context<TAB>English<TAB>translation`. An empty context marks a plain string. `@id` keys an entry by command id (for example `file.saveAs`), so one menu item can read differently from another with the same English label. `@plural` holds plural forms. Any other context disambiguates an English word with several meanings (`tl_ctx!` in UI code, for example the signature pad's "Type" button against the "Type" column). The header of `ja.tsv` documents the format and escapes.
- Placeholders (`{name}`) must appear in both columns. A trailing `…` must be kept. `i18n::fmt` fills placeholders in one pass, so a value such as a file name containing `{n}` is inserted as written.
- Catalogs are validated strictly: unknown escapes, unknown `@` contexts, placeholder or ellipsis mismatches, wrong plural form counts and duplicates are errors. The tests require the bundled catalogs to have none; at run time a bad line is skipped (and logged) and that string shows in English.
- Catalogs are assets: each `.tsv` has an `ATTRIBUTION.toml` entry with `kind = "translation"` and its SHA-256, checked by `cargo xtask assets`.
- Translations are clean-room: write them from the meaning of the English text, never from another product's string tables.

## Adding a language

1. Copy the header of `ja.tsv` into `xx.tsv` and translate entries.
2. Add one row to `LANGUAGES` in `crates/ui-egui/src/i18n/mod.rs`: code, native name, catalog and plural rule.
3. Add an `ATTRIBUTION.toml` entry for the catalog (`kind = "translation"`), then run `cargo xtask assets --write` and `cargo xtask assets`.

The Preferences dropdown, the system-language match and the catalog tests (format, duplicates, placeholders, plural forms, command ids) then pick it up.
