# Website languages

English lives in `index.html` (`data-i18n` attributes) and in `main.js` (`DYN.en`, `PH.en`). Every other language is one file here, loaded on demand when a visitor picks it.

Supported: 简体中文 `zh-CN`, 繁體中文 `zh-TW`, 日本語 `ja`, 한국어 `ko`, Español `es`, Français `fr`, Deutsch `de`, Português `pt-BR`.

## Pack shape

```js
export default {
  name, short, htmlLang,
  ui:   { 'hero.title1': '…', … },   // every data-i18n key, plus meta.title / meta.desc
  dyn:  { …, reset: c => '…', resetOn: w => '…' },   // copy used by the interactive demos
  when: [4 reset times], runs: [4 run-entry names],
  os:   { mobile, desktop: n => '…' },               // hero download button
  ph:   { pc, rail: [7 × [title, text]], s1…s7 },    // phone-remote scenes
};
```

Missing keys fall back to English. Section labels such as `< seamless handoff />` stay English in every language on purpose.

## Add a language

1. Copy a pack (for example `es.js`) and translate it.
2. Add it to `LANGS` in `main.js` (`[code, native name, short label, html lang]`) and, if the script needs it, to the font and headline rules at the end of `styles.css`.
3. Check every key against the Chinese pack: each pack must define the same keys.
